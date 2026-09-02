use std::{
  collections::BTreeMap,
  path::PathBuf,
  sync::{Arc, mpsc as sync_mpsc},
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use agentx::{
  Agent, Config, Event, EventSink, HeaderArt, HeaderCatalog, McpProfile, PermissionMode,
  PluginCatalog, PromptCatalog, RunController, RuntimeProvider, Session, SessionStore,
  SkillCatalog,
  command_palette::{SlashCommand, builtins as builtin_commands, matches as command_matches},
  protocol::{Image, ToolCall, Usage},
  run_control,
  tool::Risk,
};
use anyhow::{Context, Result};
use crossterm::event::{self, Event as InputEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
  Frame,
  layout::{Alignment, Constraint, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use tokio::{sync::mpsc, task::JoinHandle};
use uuid::Uuid;

use super::{ACCENT, ACTIVE, INK, MUTED, Term, enter_terminal, leave_terminal};
use crate::app::{expand_prompt, make_agent_with};

type RunTask = JoinHandle<(Agent<RuntimeProvider>, Session, Result<String>)>;

const BLUE: Color = Color::Rgb(24, 66, 128);
const CYAN: Color = Color::Rgb(72, 205, 214);
const YELLOW: Color = Color::Rgb(230, 199, 92);
const RED: Color = Color::Rgb(224, 103, 103);
const MAGENTA: Color = Color::Rgb(198, 118, 205);

pub(crate) struct ChatOutcome {
  pub session: Session,
  pub configure: bool,
}

enum UiEvent {
  Agent(Event),
  Approval {
    call: ToolCall,
    risk: Risk,
    reply: sync_mpsc::SyncSender<bool>,
  },
}

struct Approval {
  call: ToolCall,
  risk: Risk,
  reply: sync_mpsc::SyncSender<bool>,
}

struct CommandData {
  skills: Vec<String>,
  plugins: Vec<String>,
  mcp: Vec<String>,
  sessions: Vec<String>,
  headers: HeaderCatalog,
}

#[derive(Clone, Copy)]
enum EntryKind {
  User,
  Assistant,
  System,
  Tool,
  Error,
}

struct Entry {
  kind: EntryKind,
  text: String,
}

#[derive(Clone, Copy)]
enum AgentState {
  Running,
  Done,
  Error,
}

struct AgentView {
  state: AgentState,
  entries: Vec<Entry>,
  tools: BTreeMap<String, String>,
  assistant: Option<usize>,
  usage: Usage,
}

impl AgentView {
  fn new(state: AgentState) -> Self {
    Self {
      state,
      entries: Vec::new(),
      tools: BTreeMap::new(),
      assistant: None,
      usage: Usage::default(),
    }
  }
}

struct ChatState {
  input: String,
  primary: AgentView,
  agents: BTreeMap<String, AgentView>,
  active: Option<String>,
  roster: bool,
  scroll: u16,
  busy: bool,
  approval: Option<Approval>,
  command_selected: usize,
  splash_style: usize,
  custom_header: Option<HeaderArt>,
}

impl Default for ChatState {
  fn default() -> Self {
    Self {
      input: String::new(),
      primary: AgentView::new(AgentState::Running),
      agents: BTreeMap::new(),
      active: None,
      roster: true,
      scroll: 0,
      busy: false,
      approval: None,
      command_selected: 0,
      splash_style: select_splash_style(),
      custom_header: None,
    }
  }
}

impl ChatState {
  fn select_header(&mut self, preference: &str, catalog: &HeaderCatalog) {
    let (style, custom) = selected_header(preference, catalog);
    self.splash_style = style;
    self.custom_header = custom;
  }

  fn active_view(&self) -> &AgentView {
    self
      .active
      .as_ref()
      .and_then(|id| self.agents.get(id))
      .unwrap_or(&self.primary)
  }

  fn view_mut(&mut self, session_id: Option<&str>) -> &mut AgentView {
    match session_id {
      Some(id) => self
        .agents
        .entry(id.to_string())
        .or_insert_with(|| AgentView::new(AgentState::Running)),
      None => &mut self.primary,
    }
  }

  fn select_slot(&mut self, slot: usize) {
    self.active = if slot == 0 {
      None
    } else {
      self.agents.keys().nth(slot - 1).cloned()
    };
    self.scroll = 0;
  }

  fn cycle_agent(&mut self, forward: bool) {
    let ids: Vec<_> = self.agents.keys().cloned().collect();
    let current = self
      .active
      .as_ref()
      .and_then(|active| ids.iter().position(|id| id == active))
      .map(|index| index + 1)
      .unwrap_or(0);
    let count = ids.len() + 1;
    let next = if forward {
      (current + 1) % count
    } else {
      (current + count - 1) % count
    };
    self.active = if next == 0 {
      None
    } else {
      Some(ids[next - 1].clone())
    };
    self.scroll = 0;
  }

  fn channel(&self) -> String {
    self
      .active
      .as_ref()
      .map(|id| format!("#{}", id.chars().take(8).collect::<String>()))
      .unwrap_or_else(|| "#main".into())
  }
}

pub(crate) async fn run_chat(
  workspace: PathBuf,
  config: &mut Config,
  initial_session: Session,
  initial_prompt: Option<String>,
) -> Result<ChatOutcome> {
  let mut terminal = enter_terminal()?;
  let result = run_chat_inner(
    &mut terminal,
    workspace,
    config,
    initial_session,
    initial_prompt,
  )
  .await;
  leave_terminal(&mut terminal)?;
  result
}

async fn run_chat_inner(
  terminal: &mut Term,
  workspace: PathBuf,
  config: &mut Config,
  initial_session: Session,
  initial_prompt: Option<String>,
) -> Result<ChatOutcome> {
  let (tx, mut rx) = mpsc::unbounded_channel();
  let events_tx = tx.clone();
  let events = EventSink::new(move |event| drop(events_tx.send(UiEvent::Agent(event))));
  let approval_tx = tx;
  let approver: agentx::agent::Approver = Arc::new(move |call, risk| {
    let (reply, answer) = sync_mpsc::sync_channel(0);
    if approval_tx
      .send(UiEvent::Approval {
        call: call.clone(),
        risk,
        reply,
      })
      .is_err()
    {
      return false;
    }
    answer.recv().unwrap_or(false)
  });
  let (built_agent, mut options) =
    make_agent_with(&workspace, config, events.clone(), approver.clone()).await?;
  let prompts = PromptCatalog::discover(&workspace).await?;
  let mut commands = builtin_commands();
  for prompt in &prompts.prompts {
    if commands.iter().any(|command| command.name == prompt.name) {
      continue;
    }
    commands.push(SlashCommand::new(
      &prompt.name,
      format!("/{} [ARGS]", prompt.name),
      if prompt.description.is_empty() {
        "Run a prompt template".to_string()
      } else {
        prompt.description.clone()
      },
      "prompt",
    ));
  }
  commands.sort_by(|left, right| left.name.cmp(&right.name));
  let store = SessionStore::default_store()?;
  let plugin_catalog = PluginCatalog::discover(&workspace).await?;
  let header_catalog = HeaderCatalog::discover(&workspace).await?;
  let command_data = CommandData {
    skills: SkillCatalog::discover_with_roots(&workspace, &plugin_catalog.approved_skill_roots())
      .await?
      .skills
      .into_iter()
      .map(|skill| format!("{}  {}", skill.name, skill.description))
      .collect(),
    plugins: plugin_catalog
      .plugins
      .iter()
      .map(|plugin| {
        format!(
          "{} {}  {}",
          plugin.manifest.plugin.name,
          plugin.manifest.plugin.version,
          if plugin.approved {
            "approved"
          } else {
            "pending"
          }
        )
      })
      .collect(),
    mcp: plugin_catalog
      .merge_mcp(McpProfile::load_with(config.mcp_config.as_deref()).await?)
      .await?
      .servers
      .into_keys()
      .collect(),
    sessions: store
      .list()
      .await?
      .into_iter()
      .filter(|saved| saved.workspace == workspace)
      .map(|saved| format!("{}  {} messages", saved.id, saved.nodes.len()))
      .collect(),
    headers: header_catalog,
  };
  let mut agent = Some(built_agent);
  let mut session = Some(initial_session);
  let session_id = session.as_ref().unwrap().id;
  let (splash_style, custom_header) = selected_header(&config.ui.header, &command_data.headers);
  let mut state = ChatState {
    primary: AgentView {
      entries: session_entries(session.as_ref().unwrap()),
      usage: session.as_ref().unwrap().usage.clone(),
      ..AgentView::new(AgentState::Running)
    },
    roster: config.ui.roster_visible,
    splash_style,
    custom_header,
    ..ChatState::default()
  };
  let mut task: Option<RunTask> = None;
  let mut controller: Option<RunController> = None;
  let mut current_id = session_id;

  if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.trim().is_empty()) {
    let expanded = expand_prompt(&prompt, &prompts).await?;
    state.primary.entries.push(Entry {
      kind: EntryKind::User,
      text: expanded.clone(),
    });
    let local_agent = agent.take().unwrap();
    let mut local_session = session.take().unwrap();
    let (run_controller, mut inbox) = run_control();
    controller = Some(run_controller);
    let run_options = options.clone();
    state.busy = true;
    task = Some(tokio::spawn(async move {
      let result = local_agent
        .run_controlled(&mut local_session, expanded, run_options, &mut inbox)
        .await;
      (local_agent, local_session, result)
    }));
  }

  loop {
    while let Ok(message) = rx.try_recv() {
      apply_event(&mut state, message);
    }
    if task.as_ref().is_some_and(|task| task.is_finished()) {
      let (returned_agent, returned_session, result) = task.take().unwrap().await?;
      agent = Some(returned_agent);
      current_id = returned_session.id;
      state.primary.usage = returned_session.usage.clone();
      store.save(&returned_session).await?;
      session = Some(returned_session);
      controller = None;
      state.busy = false;
      state.primary.assistant = None;
      if let Err(error) = result {
        state.primary.entries.push(Entry {
          kind: EntryKind::Error,
          text: format!("{error:#}"),
        });
      }
    }

    terminal.draw(|frame| render(frame, &state, config, current_id, &workspace, &commands))?;
    if !event::poll(Duration::from_millis(40))? {
      continue;
    }
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    if let Some(approval) = state.approval.take() {
      match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
          let _ = approval.reply.send(true);
          state.primary.entries.push(Entry {
            kind: EntryKind::System,
            text: format!("allowed {} ({:?})", approval.call.name, approval.risk),
          });
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
          let _ = approval.reply.send(false);
          state.primary.entries.push(Entry {
            kind: EntryKind::System,
            text: format!("denied {} ({:?})", approval.call.name, approval.risk),
          });
        }
        _ => state.approval = Some(approval),
      }
      continue;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
      match key.code {
        KeyCode::Char('1') => state.select_slot(0),
        KeyCode::Char(digit @ '2'..='9') => {
          state.select_slot(digit.to_digit(10).unwrap_or(1) as usize - 1);
        }
        KeyCode::Char('+') | KeyCode::Char('=') => state.cycle_agent(true),
        KeyCode::Char('-') => state.cycle_agent(false),
        KeyCode::Char('l') => {
          state.roster = !state.roster;
          config.ui.roster_visible = state.roster;
          if let Err(error) = config.save().await {
            state.primary.entries.push(Entry {
              kind: EntryKind::Error,
              text: format!("could not remember roster setting: {error:#}"),
            });
          }
        }
        KeyCode::Char('c') => {
          if let Some(controller) = &controller {
            controller.cancel();
          } else {
            return Ok(ChatOutcome {
              session: session.context("session unavailable")?,
              configure: false,
            });
          }
        }
        _ => {}
      }
      continue;
    }
    let suggestion_count = command_matches(&commands, &state.input).len();
    if suggestion_count > 0 {
      match key.code {
        KeyCode::Up => {
          state.command_selected = if state.command_selected == 0 {
            suggestion_count - 1
          } else {
            state.command_selected - 1
          };
          continue;
        }
        KeyCode::Down => {
          state.command_selected = (state.command_selected + 1) % suggestion_count;
          continue;
        }
        KeyCode::Tab => {
          accept_command(&mut state, &commands);
          continue;
        }
        KeyCode::Esc => {
          state.input.clear();
          state.command_selected = 0;
          continue;
        }
        KeyCode::Enter if !command_is_exact(&state.input, &commands) => {
          accept_command(&mut state, &commands);
          continue;
        }
        _ => {}
      }
    }
    match key.code {
      KeyCode::Char(ch) if state.active.is_none() => {
        state.input.push(ch);
        state.command_selected = 0;
      }
      KeyCode::Backspace => {
        state.input.pop();
        state.command_selected = 0;
      }
      KeyCode::PageUp => state.scroll = state.scroll.saturating_add(8),
      KeyCode::PageDown => state.scroll = state.scroll.saturating_sub(8),
      KeyCode::Esc if state.busy => {
        if let Some(controller) = &controller {
          controller.cancel();
        }
      }
      KeyCode::Enter if state.active.is_none() && !state.input.trim().is_empty() => {
        let input = std::mem::take(&mut state.input);
        if state.busy {
          if input.trim() == "/cancel" {
            if let Some(controller) = &controller {
              controller.cancel();
            }
            state.primary.entries.push(Entry {
              kind: EntryKind::System,
              text: "cancellation requested".into(),
            });
            continue;
          }
          if input.trim_start().starts_with('/') {
            state.primary.entries.push(Entry {
              kind: EntryKind::Error,
              text: "only /cancel is available while a run is active".into(),
            });
            continue;
          }
          if let Some(controller) = &controller
            && controller.steer(input.clone())
          {
            state.primary.entries.push(Entry {
              kind: EntryKind::System,
              text: format!("steering queued: {input}"),
            });
          }
          continue;
        }
        let command_result = match command(
          &input,
          session.as_mut().unwrap(),
          &mut state,
          config,
          &commands,
          &command_data,
        ) {
          Ok(result) => result,
          Err(error) => {
            state.primary.entries.push(Entry {
              kind: EntryKind::Error,
              text: format!("{error:#}"),
            });
            continue;
          }
        };
        match command_result {
          CommandResult::Quit => {
            return Ok(ChatOutcome {
              session: session.take().unwrap(),
              configure: false,
            });
          }
          CommandResult::Configure => {
            return Ok(ChatOutcome {
              session: session.take().unwrap(),
              configure: true,
            });
          }
          CommandResult::ShowAgents => {
            state.roster = true;
            config.ui.roster_visible = true;
            if let Err(error) = config.save().await {
              state.primary.entries.push(Entry {
                kind: EntryKind::Error,
                text: format!("could not remember roster setting: {error:#}"),
              });
            }
            continue;
          }
          CommandResult::SetPermissions(mode) => {
            config.permissions = mode;
            config.save().await?;
            let (new_agent, new_options) =
              make_agent_with(&workspace, config, events.clone(), approver.clone()).await?;
            agent = Some(new_agent);
            options = new_options;
            state.primary.entries.push(Entry {
              kind: EntryKind::System,
              text: format!("permissions set to {}", permission_name(mode)),
            });
            continue;
          }
          CommandResult::SetHeader(preference) => {
            config.ui.header = preference.clone();
            state.select_header(&preference, &command_data.headers);
            config.save().await?;
            if !state.primary.entries.is_empty() {
              state.primary.entries.push(Entry {
                kind: EntryKind::System,
                text: format!("header set to {preference}; it will appear on an empty transcript"),
              });
            }
            continue;
          }
          CommandResult::Handled => {
            current_id = session.as_ref().unwrap().id;
            continue;
          }
          CommandResult::Prompt { prompt, image } => {
            let expanded = expand_prompt(&prompt, &prompts).await?;
            state.primary.entries.push(Entry {
              kind: EntryKind::User,
              text: expanded.clone(),
            });
            let images = match image {
              Some(path) => match Image::from_path(&path).await {
                Ok(image) => vec![image],
                Err(error) => {
                  state.primary.entries.push(Entry {
                    kind: EntryKind::Error,
                    text: format!("{error:#}"),
                  });
                  continue;
                }
              },
              None => Vec::new(),
            };
            let local_agent = agent.take().unwrap();
            let mut local_session = session.take().unwrap();
            let (run_controller, mut inbox) = run_control();
            controller = Some(run_controller);
            let run_options = options.clone();
            state.busy = true;
            task = Some(tokio::spawn(async move {
              let result = if images.is_empty() {
                local_agent
                  .run_controlled(&mut local_session, expanded, run_options, &mut inbox)
                  .await
              } else {
                local_agent
                  .run_controlled_with_images(
                    &mut local_session,
                    expanded,
                    images,
                    run_options,
                    &mut inbox,
                  )
                  .await
              };
              (local_agent, local_session, result)
            }));
          }
        }
      }
      _ => {}
    }
  }
}

enum CommandResult {
  Quit,
  Configure,
  ShowAgents,
  SetPermissions(PermissionMode),
  SetHeader(String),
  Handled,
  Prompt {
    prompt: String,
    image: Option<PathBuf>,
  },
}

fn command(
  input: &str,
  session: &mut Session,
  state: &mut ChatState,
  config: &Config,
  commands: &[SlashCommand],
  data: &CommandData,
) -> Result<CommandResult> {
  let input = input.trim();
  match input {
    "/exit" | "/quit" => Ok(CommandResult::Quit),
    "/config" | "/model" | "/provider" => Ok(CommandResult::Configure),
    "/agents" => Ok(CommandResult::ShowAgents),
    "/header" => {
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: format!("header: {}", config.ui.header),
      });
      Ok(CommandResult::Handled)
    }
    "/headers" => {
      let mut values = vec![
        "random  built-ins and custom art".into(),
        "builtin  built-ins only".into(),
      ];
      values.extend(data.headers.headers.iter().map(|header| {
        format!(
          "{}  {}×{}  {}",
          header.name,
          header.width,
          header.lines.len(),
          header.path.display()
        )
      }));
      values.extend(
        data
          .headers
          .issues
          .iter()
          .map(|issue| format!("invalid  {issue}")),
      );
      list_command(state, "headers", &values)
    }
    "/permissions" => {
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: format!("permissions: {}", permission_name(config.permissions)),
      });
      Ok(CommandResult::Handled)
    }
    "/cancel" => {
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: "no run is active".into(),
      });
      Ok(CommandResult::Handled)
    }
    "/help" => {
      let listing = commands
        .iter()
        .map(|command| format!("{:<28} {}", command.usage, command.description))
        .collect::<Vec<_>>()
        .join("\n");
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: format!(
          "{listing}\n\nctrl+1…9 select agent  ctrl++/- cycle  ctrl+l roster  ctrl+c cancel  page up/down scroll"
        ),
      });
      Ok(CommandResult::Handled)
    }
    "/new" | "/clear" => {
      *session = Session::new(session.workspace.clone());
      state.primary.entries.clear();
      state.primary.tools.clear();
      state.primary.assistant = None;
      state.primary.usage = Usage::default();
      state.agents.clear();
      state.active = None;
      Ok(CommandResult::Handled)
    }
    "/status" => {
      let running = state
        .agents
        .values()
        .filter(|view| matches!(view.state, AgentState::Running))
        .count();
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: format!(
          "session {}\nprovider {}\nmodel {}\npermissions {:?}\nagents {running}/{} running\nusage {} input · {} output",
          session.id,
          config.provider.as_deref().unwrap_or("default"),
          config.model,
          config.permissions,
          state.agents.len(),
          state.active_view().usage.input_tokens,
          state.active_view().usage.output_tokens,
        ),
      });
      Ok(CommandResult::Handled)
    }
    "/usage" => {
      let usage = &state.active_view().usage;
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: format!(
          "{} input · {} output · {} total tokens",
          usage.input_tokens,
          usage.output_tokens,
          usage.input_tokens.saturating_add(usage.output_tokens),
        ),
      });
      Ok(CommandResult::Handled)
    }
    "/skills" => list_command(state, "skills", &data.skills),
    "/plugins" => list_command(state, "plugins", &data.plugins),
    "/mcp" => list_command(state, "MCP servers", &data.mcp),
    "/sessions" => list_command(state, "sessions", &data.sessions),
    "/prompts" => {
      let prompts = commands
        .iter()
        .filter(|command| command.source == "prompt")
        .map(|command| format!("{}  {}", command.usage, command.description))
        .collect::<Vec<_>>();
      list_command(state, "prompts", &prompts)
    }
    "/history" => {
      for node in &session.nodes {
        state.primary.entries.push(Entry {
          kind: EntryKind::System,
          text: format!(
            "{}  {:?}  {}",
            node.id,
            node.message.role,
            node.message.content.as_deref().unwrap_or("[tool call]")
          ),
        });
      }
      Ok(CommandResult::Handled)
    }
    _ if input.starts_with("/agent ") => {
      let slot = input[7..]
        .trim()
        .parse::<usize>()
        .context("usage: /agent N")?;
      if slot == 0 || slot > state.agents.len() + 1 {
        anyhow::bail!(
          "agent number must be between 1 and {}",
          state.agents.len() + 1
        );
      }
      state.select_slot(slot - 1);
      Ok(CommandResult::Handled)
    }
    _ if input.starts_with("/permissions ") => {
      let mode = match input[13..].trim() {
        "ask" => PermissionMode::Ask,
        "auto" => PermissionMode::Auto,
        "read-only" | "readonly" => PermissionMode::ReadOnly,
        _ => anyhow::bail!("usage: /permissions ask|auto|read-only"),
      };
      Ok(CommandResult::SetPermissions(mode))
    }
    _ if input.starts_with("/header ") => {
      let preference = input[8..].trim();
      if !matches!(preference, "random" | "builtin") && data.headers.get(preference).is_none() {
        anyhow::bail!("header {preference} was not found; use /headers to list artwork");
      }
      Ok(CommandResult::SetHeader(preference.into()))
    }
    _ if input.starts_with("/checkout ") => {
      let id = input[10..]
        .trim()
        .parse::<Uuid>()
        .context("checkout node must be a UUID")?;
      session.checkout(Some(id))?;
      state.primary.entries.push(Entry {
        kind: EntryKind::System,
        text: format!("cursor {id}"),
      });
      Ok(CommandResult::Handled)
    }
    _ if input.starts_with("/image ") => {
      let value = &input[7..];
      let (path, prompt) = value
        .split_once(char::is_whitespace)
        .context("usage: /image PATH PROMPT")?;
      Ok(CommandResult::Prompt {
        prompt: prompt.trim().into(),
        image: Some(PathBuf::from(path)),
      })
    }
    _ if input.starts_with('/') => {
      let name = input[1..].split_whitespace().next().unwrap_or_default();
      if commands
        .iter()
        .any(|command| command.source == "prompt" && command.name == name)
      {
        Ok(CommandResult::Prompt {
          prompt: input.into(),
          image: None,
        })
      } else {
        anyhow::bail!("unknown command /{name}; type / to search commands")
      }
    }
    _ => Ok(CommandResult::Prompt {
      prompt: input.into(),
      image: None,
    }),
  }
}

fn list_command(state: &mut ChatState, title: &str, values: &[String]) -> Result<CommandResult> {
  state.primary.entries.push(Entry {
    kind: EntryKind::System,
    text: if values.is_empty() {
      format!("no {title} found")
    } else {
      format!("{title}\n{}", values.join("\n"))
    },
  });
  Ok(CommandResult::Handled)
}

fn permission_name(mode: PermissionMode) -> &'static str {
  match mode {
    PermissionMode::Ask => "ask",
    PermissionMode::Auto => "auto",
    PermissionMode::ReadOnly => "read-only",
  }
}

fn command_is_exact(input: &str, commands: &[SlashCommand]) -> bool {
  let Some(name) = input.strip_prefix('/') else {
    return false;
  };
  commands.iter().any(|command| command.name == name)
}

fn accept_command(state: &mut ChatState, commands: &[SlashCommand]) {
  let matches = command_matches(commands, &state.input);
  if let Some(command) = matches.get(state.command_selected.min(matches.len().saturating_sub(1))) {
    state.input = command.completion();
  }
  state.command_selected = 0;
}

fn apply_event(state: &mut ChatState, message: UiEvent) {
  match message {
    UiEvent::Approval { call, risk, reply } => {
      state.approval = Some(Approval { call, risk, reply });
    }
    UiEvent::Agent(event) => apply_agent_event(state, None, event),
  }
}

fn apply_agent_event(state: &mut ChatState, session_id: Option<&str>, event: Event) {
  match event {
    Event::SubagentEvent { session_id, event } => {
      apply_agent_event(state, Some(&session_id), *event);
    }
    Event::TextDelta { text } => {
      let view = state.view_mut(session_id);
      let index = view.assistant.unwrap_or_else(|| {
        view.entries.push(Entry {
          kind: EntryKind::Assistant,
          text: String::new(),
        });
        let index = view.entries.len() - 1;
        view.assistant = Some(index);
        index
      });
      view.entries[index].text.push_str(&text);
    }
    Event::ToolStart { call } => {
      let view = state.view_mut(session_id);
      view.tools.insert(call.id.clone(), call.name.clone());
      view.entries.push(Entry {
        kind: EntryKind::Tool,
        text: format!("running {}", call.name),
      });
    }
    Event::ToolEnd { id, error, .. } => {
      let view = state.view_mut(session_id);
      if let Some(name) = view.tools.remove(&id) {
        view.entries.push(Entry {
          kind: if error {
            EntryKind::Error
          } else {
            EntryKind::Tool
          },
          text: format!("{} {}", name, if error { "failed" } else { "complete" }),
        });
      }
    }
    Event::SubagentStart { session_id, .. } => {
      state
        .agents
        .entry(session_id)
        .or_insert_with(|| AgentView::new(AgentState::Running))
        .state = AgentState::Running;
    }
    Event::SubagentEnd { session_id, error } => {
      let view = state
        .agents
        .entry(session_id)
        .or_insert_with(|| AgentView::new(AgentState::Running));
      view.state = if error {
        AgentState::Error
      } else {
        AgentState::Done
      };
      view.assistant = None;
    }
    Event::Compaction {
      archived_messages, ..
    } => {
      state.view_mut(session_id).entries.push(Entry {
        kind: EntryKind::System,
        text: format!("compacted {archived_messages} messages"),
      });
    }
    Event::Steering { message } => state.view_mut(session_id).entries.push(Entry {
      kind: EntryKind::System,
      text: format!("steering applied: {message}"),
    }),
    Event::Cancelled => state.view_mut(session_id).entries.push(Entry {
      kind: EntryKind::System,
      text: "run cancelled".into(),
    }),
    Event::Error { message } => state.view_mut(session_id).entries.push(Entry {
      kind: EntryKind::Error,
      text: message,
    }),
    Event::TurnEnd { usage } => {
      let view = state.view_mut(session_id);
      view.usage.input_tokens += usage.input_tokens;
      view.usage.output_tokens += usage.output_tokens;
      view.assistant = None;
    }
    Event::SessionStart { .. } => {}
  }
}

fn session_entries(session: &Session) -> Vec<Entry> {
  session
    .nodes
    .iter()
    .filter_map(|node| {
      let text = node.message.content.clone()?;
      let kind = match node.message.role {
        agentx::protocol::Role::User => EntryKind::User,
        agentx::protocol::Role::Assistant => EntryKind::Assistant,
        agentx::protocol::Role::System => EntryKind::System,
        agentx::protocol::Role::Tool => EntryKind::Tool,
      };
      Some(Entry { kind, text })
    })
    .collect()
}

fn render(
  frame: &mut Frame,
  state: &ChatState,
  config: &Config,
  session_id: Uuid,
  workspace: &std::path::Path,
  commands: &[SlashCommand],
) {
  let [header, body, status, input] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Min(8),
    Constraint::Length(1),
    Constraint::Length(1),
  ])
  .areas(frame.area());
  render_title(frame, header, state, config, session_id, workspace);
  let transcript = if state.roster && body.width >= 72 {
    let [roster, transcript] = Layout::horizontal([Constraint::Length(24), Constraint::Min(30)])
      .spacing(1)
      .areas(body);
    render_roster(frame, roster, state);
    transcript
  } else {
    body
  };
  render_transcript(frame, transcript, state);
  render_status(frame, status, state, config);
  render_input(frame, input, state);
  if state.active.is_none() {
    render_command_palette(frame, transcript, input, state, commands);
  }
  if let Some(approval) = &state.approval {
    render_approval(frame, approval);
  }
}

fn render_command_palette(
  frame: &mut Frame,
  transcript: Rect,
  input: Rect,
  state: &ChatState,
  commands: &[SlashCommand],
) {
  let matches = command_matches(commands, &state.input);
  if matches.is_empty() {
    return;
  }
  let height = (matches.len().min(7) as u16 + 2).min(transcript.height);
  if height < 3 {
    return;
  }
  let area = Rect::new(
    transcript.x,
    input.y.saturating_sub(height),
    transcript.width,
    height,
  );
  let selected = state.command_selected.min(matches.len() - 1);
  let visible = height.saturating_sub(2) as usize;
  let start = selected.saturating_sub(visible.saturating_sub(1));
  let items = matches
    .iter()
    .enumerate()
    .skip(start)
    .take(visible)
    .map(|(index, command)| {
      let source = if command.source == "prompt" {
        "prompt"
      } else {
        command.source.as_str()
      };
      ListItem::new(Line::from(vec![
        Span::styled(
          format!(" {:<24}", command.usage),
          Style::default().fg(if index == selected {
            Color::White
          } else {
            CYAN
          }),
        ),
        Span::styled(
          command.description.clone(),
          Style::default().fg(if index == selected { Color::White } else { INK }),
        ),
        Span::styled(format!("  {source}"), Style::default().fg(MUTED)),
      ]))
      .style(if index == selected {
        Style::default().bg(BLUE).add_modifier(Modifier::BOLD)
      } else {
        Style::default()
      })
    })
    .collect::<Vec<_>>();
  frame.render_widget(Clear, area);
  frame.render_widget(
    List::new(items).block(
      Block::default()
        .title(" commands · ↑↓ select · tab complete · enter accept/run ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BLUE)),
    ),
    area,
  );
}

fn render_title(
  frame: &mut Frame,
  area: Rect,
  state: &ChatState,
  config: &Config,
  session: Uuid,
  workspace: &std::path::Path,
) {
  let root = workspace
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("workspace");
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(
        " AgentX",
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
      ),
      Span::styled("!", Style::default().fg(MAGENTA)),
      Span::styled(root, Style::default().fg(INK)),
      Span::styled("@", Style::default().fg(MUTED)),
      Span::styled(
        config.provider.as_deref().unwrap_or("default"),
        Style::default().fg(ACTIVE),
      ),
      Span::styled(
        format!("  {} · {} ", state.channel(), &session.to_string()[..8]),
        Style::default().fg(MUTED),
      ),
    ])),
    area,
  );
}

fn render_transcript(frame: &mut Frame, area: Rect, state: &ChatState) {
  let entries = &state.active_view().entries;
  let lines = if entries.is_empty() {
    splash(
      state.splash_style,
      state.custom_header.as_ref(),
      area.width.saturating_sub(1) as usize,
      area.height as usize,
    )
  } else {
    entries.iter().flat_map(entry_lines).collect()
  };
  let text_width = area.width.saturating_sub(1).max(1) as usize;
  let rendered_lines: usize = lines
    .iter()
    .map(|line| line.width().max(1).div_ceil(text_width))
    .sum();
  let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
    Block::default()
      .borders(Borders::LEFT)
      .border_style(Style::default().fg(BLUE)),
  );
  let bottom = rendered_lines.saturating_sub(area.height as usize);
  let scroll = bottom
    .saturating_sub(state.scroll as usize)
    .min(u16::MAX as usize) as u16;
  let paragraph = paragraph.scroll((scroll, 0));
  frame.render_widget(paragraph, area);
}

fn splash(
  style: usize,
  custom: Option<&HeaderArt>,
  width: usize,
  height: usize,
) -> Vec<Line<'static>> {
  let mut lines = custom
    .filter(|header| header.width <= width && header.lines.len().saturating_add(3) <= height)
    .map(|header| {
      header
        .lines
        .iter()
        .cloned()
        .map(|line| line.alignment(Alignment::Center))
        .collect()
    })
    .unwrap_or_else(|| pixel_masthead(width, style));
  append_splash_footer(&mut lines);
  lines
}

fn append_splash_footer(lines: &mut Vec<Line<'static>>) {
  lines.extend([
    Line::raw(""),
    Line::styled(
      "agent harness / session multiplexer",
      Style::default().fg(MUTED),
    )
    .alignment(Alignment::Center),
    Line::styled(
      "/help commands · ctrl+1…9 select · ctrl++/- cycle · ctrl+l roster",
      Style::default().fg(MUTED),
    )
    .alignment(Alignment::Center),
  ]);
}

#[derive(Clone, Copy)]
struct PixelTheme {
  face_top: Color,
  face_bottom: Color,
  highlight: Color,
  outline: Color,
  outline_dark: Color,
  shadow: Color,
  shadow_deep: Color,
}

fn pixel_masthead(width: usize, variant: usize) -> Vec<Line<'static>> {
  const LETTERS: [[&str; 7]; 6] = [
    [
      "01110", "10001", "10001", "11111", "10001", "10001", "10001",
    ],
    [
      "01110", "10001", "10000", "10111", "10001", "10001", "01110",
    ],
    [
      "11111", "10000", "10000", "11110", "10000", "10000", "11111",
    ],
    [
      "10001", "11001", "11001", "10101", "10011", "10011", "10001",
    ],
    [
      "11111", "00100", "00100", "00100", "00100", "00100", "00100",
    ],
    [
      "10001", "10001", "01010", "00100", "01010", "10001", "10001",
    ],
  ];
  const THEMES: [PixelTheme; 10] = [
    PixelTheme {
      face_top: Color::Rgb(226, 190, 48),
      face_bottom: Color::Rgb(211, 126, 29),
      highlight: Color::Rgb(255, 232, 111),
      outline: Color::Rgb(62, 188, 221),
      outline_dark: Color::Rgb(24, 44, 52),
      shadow: Color::Rgb(54, 57, 61),
      shadow_deep: Color::Rgb(27, 29, 32),
    },
    PixelTheme {
      face_top: Color::Rgb(224, 230, 229),
      face_bottom: Color::Rgb(105, 116, 122),
      highlight: Color::Rgb(255, 255, 247),
      outline: Color::Rgb(79, 224, 214),
      outline_dark: Color::Rgb(29, 42, 48),
      shadow: Color::Rgb(67, 72, 78),
      shadow_deep: Color::Rgb(25, 28, 34),
    },
    PixelTheme {
      face_top: Color::Rgb(239, 196, 49),
      face_bottom: Color::Rgb(222, 118, 31),
      highlight: Color::Rgb(255, 238, 132),
      outline: Color::Rgb(73, 209, 224),
      outline_dark: Color::Rgb(34, 31, 48),
      shadow: Color::Rgb(91, 62, 105),
      shadow_deep: Color::Rgb(31, 27, 40),
    },
    PixelTheme {
      face_top: Color::Rgb(249, 92, 38),
      face_bottom: Color::Rgb(160, 24, 28),
      highlight: Color::Rgb(255, 211, 61),
      outline: Color::Rgb(255, 142, 28),
      outline_dark: Color::Rgb(65, 17, 25),
      shadow: Color::Rgb(91, 25, 38),
      shadow_deep: Color::Rgb(31, 13, 22),
    },
    PixelTheme {
      face_top: Color::Rgb(137, 237, 52),
      face_bottom: Color::Rgb(39, 143, 69),
      highlight: Color::Rgb(222, 255, 96),
      outline: Color::Rgb(188, 55, 217),
      outline_dark: Color::Rgb(39, 21, 52),
      shadow: Color::Rgb(67, 34, 83),
      shadow_deep: Color::Rgb(20, 17, 30),
    },
    PixelTheme {
      face_top: Color::Rgb(208, 251, 255),
      face_bottom: Color::Rgb(78, 169, 224),
      highlight: Color::Rgb(255, 255, 255),
      outline: Color::Rgb(45, 116, 222),
      outline_dark: Color::Rgb(22, 38, 72),
      shadow: Color::Rgb(38, 73, 127),
      shadow_deep: Color::Rgb(16, 25, 48),
    },
    PixelTheme {
      face_top: Color::Rgb(255, 86, 209),
      face_bottom: Color::Rgb(137, 55, 198),
      highlight: Color::Rgb(255, 191, 239),
      outline: Color::Rgb(54, 231, 225),
      outline_dark: Color::Rgb(38, 20, 68),
      shadow: Color::Rgb(51, 49, 122),
      shadow_deep: Color::Rgb(20, 18, 44),
    },
    PixelTheme {
      face_top: Color::Rgb(246, 199, 53),
      face_bottom: Color::Rgb(176, 111, 22),
      highlight: Color::Rgb(255, 238, 145),
      outline: Color::Rgb(116, 123, 127),
      outline_dark: Color::Rgb(34, 37, 39),
      shadow: Color::Rgb(62, 66, 68),
      shadow_deep: Color::Rgb(21, 23, 24),
    },
    PixelTheme {
      face_top: Color::Rgb(66, 221, 190),
      face_bottom: Color::Rgb(26, 111, 156),
      highlight: Color::Rgb(166, 255, 228),
      outline: Color::Rgb(62, 94, 222),
      outline_dark: Color::Rgb(17, 31, 65),
      shadow: Color::Rgb(30, 56, 105),
      shadow_deep: Color::Rgb(12, 21, 43),
    },
    PixelTheme {
      face_top: Color::Rgb(211, 207, 197),
      face_bottom: Color::Rgb(102, 99, 96),
      highlight: Color::Rgb(255, 250, 232),
      outline: Color::Rgb(196, 46, 54),
      outline_dark: Color::Rgb(41, 28, 30),
      shadow: Color::Rgb(62, 58, 58),
      shadow_deep: Color::Rgb(19, 18, 19),
    },
  ];

  fn put(canvas: &mut [Vec<Option<Color>>], x: isize, y: isize, color: Color) {
    if x >= 0
      && y >= 0
      && let Some(row) = canvas.get_mut(y as usize)
      && let Some(pixel) = row.get_mut(x as usize)
    {
      *pixel = Some(color);
    }
  }

  fn face_color(
    theme: PixelTheme,
    variant: usize,
    letter: usize,
    x: usize,
    y: usize,
    height: usize,
  ) -> Color {
    if (x * 17 + y * 29 + letter * 11).is_multiple_of(19) {
      return theme.highlight;
    }
    match variant {
      1 if letter == 3 || (x + y + letter).is_multiple_of(17) => {
        return Color::Rgb(67, 213, 230);
      }
      1 if (x * 3 + y + letter).is_multiple_of(23) => {
        return Color::Rgb(119, 239, 83);
      }
      4 if (x + y * 2 + letter).is_multiple_of(13) => {
        return Color::Rgb(208, 66, 221);
      }
      5 if (x * 2 + y + letter).is_multiple_of(11) => {
        return Color::Rgb(116, 237, 255);
      }
      6 if letter.is_multiple_of(2) && (x + y).is_multiple_of(7) => {
        return Color::Rgb(61, 223, 228);
      }
      7 if (x + y + letter).is_multiple_of(9) => {
        return Color::Rgb(75, 78, 78);
      }
      8 if (x * 3 + y + letter).is_multiple_of(11) => {
        return Color::Rgb(58, 102, 226);
      }
      9 if (x + y * 3 + letter).is_multiple_of(17) => {
        return Color::Rgb(205, 47, 57);
      }
      _ => {}
    }
    if y < height / 2 {
      theme.face_top
    } else {
      theme.face_bottom
    }
  }

  let variant = variant % 10;
  let theme = THEMES[variant];
  let scale = if width >= 78 { 2 } else { 1 };
  let letter_width = 5 * scale;
  let gap = 1;
  let face_width = LETTERS.len() * letter_width + (LETTERS.len() - 1) * gap;
  let face_height = 7 * scale;
  let depth = if scale == 2 { 3 } else { 2 };
  let margin = depth + 2;
  let top = if scale == 2 { 6 } else { 4 };
  let canvas_width = face_width + margin * 2;
  let canvas_height = top + face_height + depth + if scale == 2 { 5 } else { 3 };
  let mut canvas = vec![vec![None; canvas_width]; canvas_height];
  let mut face = Vec::new();

  for (letter, pattern) in LETTERS.iter().enumerate() {
    let letter_x = margin + letter * (letter_width + gap);
    let letter_y = match variant {
      2 | 8 => [1, 0, 2, 0, 1, 0][letter] * scale / 2,
      4 => [0, 1, 2, 1, 0, 1][letter] * scale / 2,
      9 => [1, 0, 1, 0, 1, 0][letter] * scale / 2,
      _ => 0,
    };
    for (pattern_y, row) in pattern.iter().enumerate() {
      for (pattern_x, bit) in row.bytes().enumerate() {
        if bit != b'1' {
          continue;
        }
        for dy in 0..scale {
          for dx in 0..scale {
            let slant = match variant {
              1 | 6 => (6 - pattern_y) * scale / 4,
              3 => pattern_y * scale / 6,
              _ => 0,
            };
            face.push((
              (letter_x + pattern_x * scale + dx + slant) as isize,
              (top + letter_y + pattern_y * scale + dy) as isize,
              letter,
            ));
          }
        }
      }
    }
  }

  for &(x, y, _) in &face {
    for step in 0..=depth as isize {
      for oy in -1..=1 {
        for ox in -1..=1 {
          let direction = if matches!(variant, 3 | 6 | 9) { -1 } else { 1 };
          put(
            &mut canvas,
            x + step * direction + ox,
            y + step + oy,
            theme.outline,
          );
        }
      }
    }
  }

  for &(x, y, _) in &face {
    for step in 1..=depth as isize {
      let direction = if matches!(variant, 3 | 6 | 9) { -1 } else { 1 };
      put(
        &mut canvas,
        x + step * direction,
        y + step,
        if step == depth as isize {
          theme.shadow_deep
        } else {
          theme.shadow
        },
      );
    }
  }

  for &(x, y, _) in &face {
    for oy in -1..=1 {
      for ox in -1..=1 {
        put(&mut canvas, x + ox, y + oy, theme.outline_dark);
      }
    }
  }

  for &(x, y, letter) in &face {
    let color = face_color(
      theme,
      variant,
      letter,
      x as usize,
      y.saturating_sub(top as isize) as usize,
      face_height,
    );
    put(&mut canvas, x, y, color);
  }

  if matches!(variant, 0 | 2 | 4 | 5 | 6 | 8) {
    let drip_columns = [1, 8, 15, 24, 31];
    for (index, column) in drip_columns.into_iter().enumerate() {
      let x = margin + column * scale;
      let start = top + face_height + depth;
      let length = 1 + (index * 2 + variant) % if scale == 2 { 5 } else { 3 };
      for dy in 0..length {
        put(
          &mut canvas,
          x as isize,
          (start + dy) as isize,
          if dy + 1 == length {
            theme.highlight
          } else {
            theme.outline
          },
        );
        if scale == 2 && dy < 2 {
          put(
            &mut canvas,
            x as isize + 1,
            (start + dy) as isize,
            theme.shadow,
          );
        }
      }
    }
  }

  match variant {
    0 => paint_flame(&mut canvas, canvas_width / 2, theme.outline_dark),
    1 => paint_shards(&mut canvas, canvas_width, theme),
    2 => paint_sparks(&mut canvas, canvas_width, theme),
    3 => paint_inferno(&mut canvas, canvas_width, theme),
    4 => paint_toxic(&mut canvas, canvas_width, theme),
    5 => paint_ice(&mut canvas, canvas_width, theme),
    6 => paint_orbit(&mut canvas, canvas_width, theme),
    7 => paint_industrial(&mut canvas, canvas_width, theme),
    8 => paint_abyss(&mut canvas, canvas_width, theme),
    _ => paint_skull(&mut canvas, canvas_width, theme),
  }

  let left_padding = width.saturating_sub(canvas_width) / 2;
  let mut lines = vec![Line::raw("")];
  for rows in canvas.chunks(2) {
    let top_row = &rows[0];
    let bottom_row = rows.get(1);
    let mut spans = vec![Span::raw(" ".repeat(left_padding))];
    for x in 0..canvas_width {
      let upper = top_row[x];
      let lower = bottom_row.and_then(|row| row[x]);
      spans.push(match (upper, lower) {
        (Some(fg), Some(bg)) => Span::styled("▀", Style::default().fg(fg).bg(bg)),
        (Some(fg), None) => Span::styled("▀", Style::default().fg(fg)),
        (None, Some(fg)) => Span::styled("▄", Style::default().fg(fg)),
        (None, None) => Span::raw(" "),
      });
    }
    lines.push(Line::from(spans));
  }
  lines
}

fn paint_flame(canvas: &mut [Vec<Option<Color>>], center: usize, edge: Color) {
  const FLAME: [&str; 5] = ["..r....", ".rr..r.", ".rorrr.", "..ryr..", "...r..."];
  let left = center.saturating_sub(FLAME[0].len() / 2);
  for (y, row) in FLAME.iter().enumerate() {
    for (x, pixel) in row.chars().enumerate() {
      if pixel == '.' {
        continue;
      }
      for oy in -1..=1 {
        for ox in -1..=1 {
          let px = (left + x) as isize + ox;
          let py = y as isize + oy;
          if px >= 0
            && py >= 0
            && let Some(line) = canvas.get_mut(py as usize)
            && let Some(cell) = line.get_mut(px as usize)
          {
            *cell = Some(edge);
          }
        }
      }
    }
  }
  for (y, row) in FLAME.iter().enumerate() {
    for (x, pixel) in row.chars().enumerate() {
      let color = match pixel {
        'r' => Some(Color::Rgb(210, 76, 72)),
        'o' => Some(Color::Rgb(244, 120, 31)),
        'y' => Some(Color::Rgb(255, 213, 73)),
        _ => None,
      };
      if let Some(color) = color
        && let Some(line) = canvas.get_mut(y)
        && let Some(cell) = line.get_mut(left + x)
      {
        *cell = Some(color);
      }
    }
  }
}

fn paint_shards(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let shards = [
    (width / 5, 1, 3),
    (width / 3, 0, 4),
    (width * 2 / 3, 1, 3),
    (width * 4 / 5, 0, 4),
  ];
  for (x, y, length) in shards {
    for step in 0..length {
      if let Some(row) = canvas.get_mut(y + step)
        && let Some(cell) = row.get_mut(x + step / 2)
      {
        *cell = Some(if step == 0 {
          theme.highlight
        } else {
          theme.outline
        });
      }
    }
  }
}

fn paint_sparks(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let sparks = [
    (width / 7, 2),
    (width / 4, 0),
    (width / 2, 2),
    (width * 3 / 4, 1),
    (width * 6 / 7, 3),
  ];
  for (index, (x, y)) in sparks.into_iter().enumerate() {
    if let Some(row) = canvas.get_mut(y)
      && let Some(cell) = row.get_mut(x)
    {
      *cell = Some(if index.is_multiple_of(2) {
        theme.highlight
      } else {
        theme.outline
      });
    }
  }
}

fn scene_put(canvas: &mut [Vec<Option<Color>>], x: usize, y: usize, color: Color) {
  if let Some(row) = canvas.get_mut(y)
    && let Some(cell) = row.get_mut(x)
  {
    *cell = Some(color);
  }
}

fn paint_inferno(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  for x in 1..width.saturating_sub(1) {
    let height = 1 + (x * 7 % 5);
    if x % 3 == 0 {
      for y in 0..height {
        scene_put(
          canvas,
          x,
          5_usize.saturating_sub(y),
          if y + 1 == height {
            theme.highlight
          } else {
            theme.face_top
          },
        );
      }
    }
  }
}

fn paint_toxic(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let bubbles = [
    (width / 8, 2, 2),
    (width / 3, 0, 1),
    (width * 2 / 3, 1, 2),
    (width * 7 / 8, 0, 1),
  ];
  for (cx, cy, radius) in bubbles {
    for y in 0..=radius * 2 {
      for x in 0..=radius * 2 {
        let edge = x == 0 || y == 0 || x == radius * 2 || y == radius * 2;
        if edge {
          scene_put(canvas, cx + x - radius, cy + y, theme.outline);
        }
      }
    }
    scene_put(canvas, cx, cy + radius, theme.highlight);
  }
}

fn paint_ice(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  for (index, x) in [
    width / 9,
    width / 4,
    width / 2,
    width * 3 / 4,
    width * 8 / 9,
  ]
  .into_iter()
  .enumerate()
  {
    let length = 2 + index % 4;
    for step in 0..length {
      scene_put(
        canvas,
        x + step / 2,
        step,
        if step == 0 {
          theme.highlight
        } else {
          theme.outline
        },
      );
      if x > step / 2 {
        scene_put(canvas, x - step / 2, step, theme.face_bottom);
      }
    }
  }
}

fn paint_orbit(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let center = width / 2;
  for offset in 0..center.saturating_sub(3) {
    if offset % 3 == 0 {
      let y = (offset * 5 / center.max(1)).min(4);
      scene_put(canvas, center + offset, y, theme.outline);
      scene_put(canvas, center - offset, 4 - y, theme.face_top);
    }
  }
  for y in 0..4 {
    scene_put(canvas, center, y, theme.highlight);
    if center + 1 < width {
      scene_put(canvas, center + 1, y, theme.face_bottom);
    }
  }
}

fn paint_industrial(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  for x in 1..width.saturating_sub(1) {
    if x % 4 < 2 {
      scene_put(canvas, x, 1, theme.face_top);
      scene_put(canvas, x, 2, theme.outline_dark);
    }
  }
  for x in (4..width.saturating_sub(4)).step_by(9) {
    scene_put(canvas, x, 4, theme.highlight);
    scene_put(canvas, x + 1, 4, theme.shadow);
  }
}

fn paint_abyss(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  let center = width / 2;
  for x in center.saturating_sub(7)..=(center + 7).min(width.saturating_sub(1)) {
    let distance = x.abs_diff(center);
    let y = distance / 3;
    scene_put(canvas, x, y, theme.outline);
    scene_put(canvas, x, 5_usize.saturating_sub(y), theme.face_bottom);
  }
  scene_put(canvas, center, 2, theme.highlight);
  scene_put(canvas, center, 3, theme.shadow_deep);
  for x in [2, width / 6, width * 5 / 6, width.saturating_sub(3)] {
    for y in 1..5 {
      scene_put(canvas, x + y % 2, y, theme.outline);
    }
  }
}

fn paint_skull(canvas: &mut [Vec<Option<Color>>], width: usize, theme: PixelTheme) {
  const SKULL: [&str; 6] = [
    ".xxxxx.", "xx...xx", "x.x.x.x", "xx...xx", ".xxxxx.", "..x.x..",
  ];
  let left = width.saturating_sub(SKULL[0].len()) / 2;
  for (y, row) in SKULL.iter().enumerate() {
    for (x, pixel) in row.chars().enumerate() {
      if pixel == 'x' {
        scene_put(
          canvas,
          left + x,
          y,
          if y == 2 && (x == 2 || x == 4) {
            theme.outline
          } else {
            theme.face_top
          },
        );
      }
    }
  }
}

fn select_splash_style() -> usize {
  let seed = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
    ^ u128::from(std::process::id());
  (seed % 10) as usize
}

fn selected_header(preference: &str, catalog: &HeaderCatalog) -> (usize, Option<HeaderArt>) {
  let style = select_splash_style();
  match preference {
    "builtin" => (style, None),
    "random" if !catalog.headers.is_empty() => {
      let choice = select_header_index(10 + catalog.headers.len());
      if choice < 10 {
        (choice, None)
      } else {
        (style, Some(catalog.headers[choice - 10].clone()))
      }
    }
    "random" => (style, None),
    name => (style, catalog.get(name).cloned()),
  }
}

fn select_header_index(count: usize) -> usize {
  let seed = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
    ^ u128::from(std::process::id());
  (seed % count.max(1) as u128) as usize
}

fn entry_lines(entry: &Entry) -> Vec<Line<'static>> {
  let (nick, color) = match entry.kind {
    EntryKind::User => ("you", CYAN),
    EntryKind::Assistant => ("AgentX", ACTIVE),
    EntryKind::System => ("*", YELLOW),
    EntryKind::Tool => ("tool", MAGENTA),
    EntryKind::Error => ("error", RED),
  };
  entry
    .text
    .lines()
    .enumerate()
    .map(|(index, text)| {
      if index == 0 {
        Line::from(vec![
          Span::styled(format!(" {} ", clock()), Style::default().fg(MUTED)),
          Span::styled(
            format!("<{nick}> "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
          ),
          Span::styled(text.to_string(), Style::default().fg(INK)),
        ])
      } else {
        Line::from(vec![
          Span::raw("              "),
          Span::styled(text.to_string(), Style::default().fg(INK)),
        ])
      }
    })
    .collect()
}

fn render_roster(frame: &mut Frame, area: Rect, state: &ChatState) {
  let primary_style = if state.active.is_none() {
    Style::default().fg(Color::White).bg(BLUE)
  } else {
    Style::default()
  };
  let mut items = vec![
    ListItem::new(Line::from(vec![
      Span::styled("1 @ ", Style::default().fg(ACTIVE)),
      Span::styled(
        "primary",
        Style::default().fg(INK).add_modifier(Modifier::BOLD),
      ),
    ]))
    .style(primary_style),
  ];
  items.extend(state.agents.iter().enumerate().map(|(index, (id, view))| {
    let (marker, color) = match view.state {
      AgentState::Running => ("+", CYAN),
      AgentState::Done => (" ", MUTED),
      AgentState::Error => ("!", RED),
    };
    let selected = state.active.as_deref() == Some(id.as_str());
    let slot = index + 2;
    let key = if slot <= 9 {
      slot.to_string()
    } else {
      "·".into()
    };
    ListItem::new(Line::from(vec![
      Span::styled(format!("{key} {marker} "), Style::default().fg(color)),
      Span::styled(
        id.chars().take(10).collect::<String>(),
        Style::default().fg(color),
      ),
    ]))
    .style(if selected {
      Style::default().fg(Color::White).bg(BLUE)
    } else {
      Style::default()
    })
  }));
  if items.len() == 1 {
    items.push(ListItem::new(Line::styled(
      "  no subagents",
      Style::default().fg(MUTED),
    )));
  }
  frame.render_widget(
    List::new(items).block(
      Block::default()
        .title(" agents ")
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(BLUE)),
    ),
    area,
  );
}

fn render_status(frame: &mut Frame, area: Rect, state: &ChatState, config: &Config) {
  let running = state
    .agents
    .values()
    .filter(|view| matches!(view.state, AgentState::Running))
    .count();
  let total_agents = state.agents.len();
  let view = state.active_view();
  let input = compact_count(view.usage.input_tokens);
  let output = compact_count(view.usage.output_tokens);
  let total = compact_count(
    view
      .usage
      .input_tokens
      .saturating_add(view.usage.output_tokens),
  );
  let provider = config.provider.as_deref().unwrap_or("default");
  let permissions = match config.permissions {
    PermissionMode::Ask => "ask",
    PermissionMode::Auto => "auto",
    PermissionMode::ReadOnly => "read-only",
  };
  let activity = view
    .tools
    .values()
    .next()
    .map(|tool| format!("tool:{tool}"))
    .unwrap_or_else(|| {
      if state.active.is_some() {
        match view.state {
          AgentState::Running => "working".into(),
          AgentState::Done => "done".into(),
          AgentState::Error => "error".into(),
        }
      } else if state.busy {
        "thinking".into()
      } else {
        "ready".into()
      }
    });
  let text = if area.width >= 108 {
    format!(
      " {provider}/{} │ {permissions} │ tokens in {input} out {output} total {total} │ agents {running}/{total_agents} │ {activity} │ ^L roster ",
      config.model,
    )
  } else if area.width >= 72 {
    format!(
      " {provider}/{} │ {permissions} │ tok {input}↓ {output}↑ Σ{total} │ ag {running}/{total_agents} │ {activity} ",
      config.model,
    )
  } else {
    format!(" {} │ Σ{total} tok │ {activity} ", config.model)
  };
  frame.render_widget(
    Paragraph::new(text).style(
      Style::default()
        .fg(Color::White)
        .bg(BLUE)
        .add_modifier(Modifier::BOLD),
    ),
    area,
  );
}

fn compact_count(value: u64) -> String {
  match value {
    0..=999 => value.to_string(),
    1_000..=999_999 => compact_scaled(value, 1_000, "k"),
    _ => compact_scaled(value, 1_000_000, "m"),
  }
}

fn compact_scaled(value: u64, scale: u64, suffix: &str) -> String {
  let whole = value / scale;
  let decimal = value % scale / (scale / 10);
  if decimal == 0 || whole >= 100 {
    format!("{whole}{suffix}")
  } else {
    format!("{whole}.{decimal}{suffix}")
  }
}

fn render_input(frame: &mut Frame, area: Rect, state: &ChatState) {
  if state.active.is_some() {
    frame.render_widget(
      Paragraph::new(Line::from(vec![
        Span::styled("· ", Style::default().fg(MUTED)),
        Span::styled(
          "subagent transcript · view only · ctrl+1 returns to primary",
          Style::default().fg(MUTED),
        ),
      ])),
      area,
    );
    return;
  }
  let prefix = if state.busy { "↳ " } else { "> " };
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(
        prefix,
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
      ),
      Span::styled(state.input.clone(), Style::default().fg(INK)),
    ])),
    area,
  );
  let x = area
    .x
    .saturating_add(2)
    .saturating_add(state.input.chars().count().min(u16::MAX as usize) as u16)
    .min(area.right().saturating_sub(1));
  frame.set_cursor_position((x, area.y));
}

fn render_approval(frame: &mut Frame, approval: &Approval) {
  let area = centered(frame.area(), frame.area().width.min(68), 7);
  frame.render_widget(Clear, area);
  frame.render_widget(
    Paragraph::new(vec![
      Line::styled(
        "permission required",
        Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
      ),
      Line::raw(""),
      Line::from(vec![
        Span::styled(
          &approval.call.name,
          Style::default().fg(INK).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {:?}", approval.risk), Style::default().fg(MUTED)),
      ]),
      Line::styled("y allow   n deny", Style::default().fg(ACCENT)),
    ])
    .block(
      Block::default()
        .title(" approval ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(YELLOW))
        .padding(ratatui::widgets::Padding::horizontal(2)),
    ),
    area,
  );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
  let [area] = Layout::horizontal([Constraint::Length(width)])
    .flex(ratatui::layout::Flex::Center)
    .areas(area);
  let [area] = Layout::vertical([Constraint::Length(height)])
    .flex(ratatui::layout::Flex::Center)
    .areas(area);
  area
}

fn clock() -> String {
  let seconds = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
    % 86_400;
  format!("{:02}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}
