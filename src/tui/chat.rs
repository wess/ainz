use std::{
  cell::RefCell,
  collections::BTreeMap,
  path::PathBuf,
  sync::Arc,
  time::{SystemTime, UNIX_EPOCH},
};

use agentx::{
  Agent, Approver, Config, Event, EventSink, HeaderArt, HeaderCatalog, McpProfile, PermissionMode,
  PluginCatalog, PromptCatalog, RunController, RuntimeProvider, Session, SessionStore,
  SkillCatalog,
  command_palette::{SlashCommand, builtins as builtin_commands, matches as command_matches},
  protocol::{Image, ToolCall, Usage},
  run_control,
  tool::Risk,
};
use anyhow::{Context, Result};
use crossterm::event::{Event as InputEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::{
  Frame,
  layout::{Alignment, Constraint, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
};
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};
use uuid::Uuid;

use super::{
  ACCENT, ACTIVE, BLUE, CYAN, INK, MAGENTA, MUTED, RED, Term, YELLOW, centered, enter_terminal,
  leave_terminal, masthead,
};
use crate::app::{expand_prompt, make_agent_with};

type RunOutput = (Agent<RuntimeProvider>, Session, Result<String>);
type RunTask = JoinHandle<RunOutput>;

pub(crate) struct ChatOutcome {
  pub session: Session,
  pub configure: bool,
}

enum UiEvent {
  Agent(Event),
  Approval(Approval),
}

struct Approval {
  call: ToolCall,
  risk: Risk,
  reply: oneshot::Sender<bool>,
}

// whatever woke the loop; owning the payload keeps the select! borrows out of the handlers
enum Wake {
  Agent(Option<UiEvent>),
  Finished(Box<Result<RunOutput>>),
  Input(Option<std::io::Result<InputEvent>>),
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
  at: String,
}

impl Entry {
  fn new(kind: EntryKind, text: String) -> Self {
    Self {
      kind,
      text,
      at: clock(),
    }
  }
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
  // set by the first ctrl+c of a run; the second abandons the run
  cancelled: bool,
  approval: Option<Approval>,
  command_selected: usize,
  splash_style: usize,
  custom_header: Option<HeaderArt>,
  // the splash is thousands of cell puts that depend only on size and choice; paint it once
  splash_cache: RefCell<Option<(usize, usize, Vec<Line<'static>>)>>,
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
      cancelled: false,
      approval: None,
      command_selected: 0,
      splash_style: 0,
      custom_header: None,
      splash_cache: RefCell::new(None),
    }
  }
}

impl ChatState {
  fn select_header(&mut self, preference: &str, catalog: &HeaderCatalog) {
    let (style, custom) = selected_header(preference, catalog);
    self.splash_style = style;
    self.custom_header = custom;
    self.splash_cache.replace(None);
  }

  fn splash(&self, width: usize, height: usize) -> Vec<Line<'static>> {
    let mut cache = self.splash_cache.borrow_mut();
    if let Some((cached_width, cached_height, lines)) = &*cache
      && (*cached_width, *cached_height) == (width, height)
    {
      return lines.clone();
    }
    let lines = splash(
      self.splash_style,
      self.custom_header.as_ref(),
      width,
      height,
    );
    *cache = Some((width, height, lines.clone()));
    lines
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
  let approver: Approver = Arc::new(move |call, risk| {
    let (reply, answer) = oneshot::channel();
    let sent = approval_tx
      .send(UiEvent::Approval(Approval {
        call: call.clone(),
        risk,
        reply,
      }))
      .is_ok();
    Box::pin(async move { sent && answer.await.unwrap_or(false) })
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
      .map(|saved| format!("{}  {} messages", saved.id, saved.nodes))
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
    state
      .primary
      .entries
      .push(Entry::new(EntryKind::User, expanded.clone()));
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

  let mut input = EventStream::new();
  loop {
    while let Ok(message) = rx.try_recv() {
      apply_event(&mut state, message);
    }
    terminal.draw(|frame| render(frame, &state, config, current_id, &workspace, &commands))?;
    let wake = tokio::select! {
      message = rx.recv() => Wake::Agent(message),
      joined = join(task.as_mut()), if task.is_some() => Wake::Finished(Box::new(joined)),
      event = input.next() => Wake::Input(event),
    };
    let key = match wake {
      Wake::Agent(Some(message)) => {
        apply_event(&mut state, message);
        continue;
      }
      Wake::Agent(None) => continue,
      Wake::Finished(joined) => {
        task = None;
        // events emitted just before the run returned are still queued; show them first
        while let Ok(message) = rx.try_recv() {
          apply_event(&mut state, message);
        }
        let (returned_agent, returned_session, result) = (*joined)?;
        agent = Some(returned_agent);
        current_id = returned_session.id;
        state.primary.usage = returned_session.usage.clone();
        store.save(&returned_session).await?;
        session = Some(returned_session);
        controller = None;
        state.busy = false;
        state.cancelled = false;
        state.primary.assistant = None;
        state.primary.tools.clear();
        if let Err(error) = result {
          state
            .primary
            .entries
            .push(Entry::new(EntryKind::Error, format!("{error:#}")));
        }
        continue;
      }
      Wake::Input(Some(Ok(InputEvent::Key(key)))) if key.kind != KeyEventKind::Release => key,
      Wake::Input(Some(Ok(InputEvent::Paste(text)))) => {
        if state.active.is_none() && state.approval.is_none() {
          state.input.push_str(&text.replace(['\r', '\n'], " "));
          state.command_selected = 0;
        }
        continue;
      }
      Wake::Input(Some(Ok(_))) => continue,
      Wake::Input(Some(Err(error))) => return Err(error).context("read terminal input"),
      Wake::Input(None) => anyhow::bail!("terminal input closed"),
    };
    if let Some(approval) = state.approval.take() {
      match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
          let _ = approval.reply.send(true);
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            format!("allowed {} ({:?})", approval.call.name, approval.risk),
          ));
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
          let _ = approval.reply.send(false);
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            format!("denied {} ({:?})", approval.call.name, approval.risk),
          ));
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
          let visible = !state.roster;
          set_roster(&mut state, config, visible).await;
        }
        KeyCode::Char('c') => match &controller {
          Some(controller) if !state.cancelled => {
            controller.cancel();
            state.cancelled = true;
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              "cancelling; press ctrl+c again to abandon the run".into(),
            ));
          }
          // the provider is ignoring cancel: abandon the run and fall back to the last save
          Some(_) => {
            if let Some(task) = task.take() {
              task.abort();
            }
            let session = match store.load(current_id).await {
              Ok(session) => session,
              Err(_) => Session::new(workspace.clone()),
            };
            return Ok(ChatOutcome {
              session,
              configure: false,
            });
          }
          None => {
            return Ok(ChatOutcome {
              session: session.context("session unavailable")?,
              configure: false,
            });
          }
        },
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
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              "cancellation requested".into(),
            ));
            continue;
          }
          if input.trim_start().starts_with('/') {
            state.primary.entries.push(Entry::new(
              EntryKind::Error,
              "only /cancel is available while a run is active".into(),
            ));
            continue;
          }
          if let Some(controller) = &controller
            && controller.steer(input.clone())
          {
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              format!("steering queued: {input}"),
            ));
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
            state
              .primary
              .entries
              .push(Entry::new(EntryKind::Error, format!("{error:#}")));
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
            set_roster(&mut state, config, true).await;
            continue;
          }
          CommandResult::SetPermissions(mode) => {
            config.permissions = mode;
            config.save().await?;
            let (new_agent, new_options) =
              make_agent_with(&workspace, config, events.clone(), approver.clone()).await?;
            agent = Some(new_agent);
            options = new_options;
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              format!("permissions set to {}", permission_name(mode)),
            ));
            continue;
          }
          CommandResult::SetHeader(preference) => {
            config.ui.header = preference.clone();
            state.select_header(&preference, &command_data.headers);
            config.save().await?;
            if !state.primary.entries.is_empty() {
              state.primary.entries.push(Entry::new(
                EntryKind::System,
                format!("header set to {preference}; it will appear on an empty transcript"),
              ));
            }
            continue;
          }
          CommandResult::Handled => {
            current_id = session.as_ref().unwrap().id;
            continue;
          }
          CommandResult::Prompt { prompt, image } => {
            let expanded = expand_prompt(&prompt, &prompts).await?;
            state
              .primary
              .entries
              .push(Entry::new(EntryKind::User, expanded.clone()));
            let images = match image {
              Some(path) => match Image::from_path(&path).await {
                Ok(image) => vec![image],
                Err(error) => {
                  state
                    .primary
                    .entries
                    .push(Entry::new(EntryKind::Error, format!("{error:#}")));
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

async fn join(task: Option<&mut RunTask>) -> Result<RunOutput> {
  match task {
    Some(task) => task.await.context("agent task failed"),
    None => std::future::pending().await,
  }
}

async fn set_roster(state: &mut ChatState, config: &mut Config, visible: bool) {
  state.roster = visible;
  config.ui.roster_visible = visible;
  if let Err(error) = config.save().await {
    state.primary.entries.push(Entry::new(
      EntryKind::Error,
      format!("could not remember roster setting: {error:#}"),
    ));
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
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        format!("header: {}", config.ui.header),
      ));
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
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        format!("permissions: {}", permission_name(config.permissions)),
      ));
      Ok(CommandResult::Handled)
    }
    "/cancel" => {
      state
        .primary
        .entries
        .push(Entry::new(EntryKind::System, "no run is active".into()));
      Ok(CommandResult::Handled)
    }
    "/help" => {
      let listing = commands
        .iter()
        .map(|command| format!("{:<28} {}", command.usage, command.description))
        .collect::<Vec<_>>()
        .join("\n");
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        format!(
          "{listing}\n\nctrl+1…9 select agent  ctrl+= / ctrl+- cycle  ctrl+l roster  \
           ctrl+c cancel  page up/down scroll"
        ),
      ));
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
      let usage = &state.active_view().usage;
      let text = format!(
        "session {}\nprovider {}\nmodel {}\npermissions {}\nagents {running}/{} running\n\
         usage {} input · {} output",
        session.id,
        config.provider.as_deref().unwrap_or("default"),
        config.model,
        permission_name(config.permissions),
        state.agents.len(),
        usage.input_tokens,
        usage.output_tokens,
      );
      state
        .primary
        .entries
        .push(Entry::new(EntryKind::System, text));
      Ok(CommandResult::Handled)
    }
    "/usage" => {
      let usage = &state.active_view().usage;
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        format!(
          "{} input · {} output · {} total tokens",
          usage.input_tokens,
          usage.output_tokens,
          usage.input_tokens.saturating_add(usage.output_tokens),
        ),
      ));
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
        state.primary.entries.push(Entry::new(
          EntryKind::System,
          format!(
            "{}  {:?}  {}",
            node.id,
            node.message.role,
            node.message.content.as_deref().unwrap_or("[tool call]")
          ),
        ));
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
      state
        .primary
        .entries
        .push(Entry::new(EntryKind::System, format!("cursor {id}")));
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
  state.primary.entries.push(Entry::new(
    EntryKind::System,
    if values.is_empty() {
      format!("no {title} found")
    } else {
      format!("{title}\n{}", values.join("\n"))
    },
  ));
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
    UiEvent::Approval(approval) => state.approval = Some(approval),
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
        view
          .entries
          .push(Entry::new(EntryKind::Assistant, String::new()));
        let index = view.entries.len() - 1;
        view.assistant = Some(index);
        index
      });
      view.entries[index].text.push_str(&text);
    }
    Event::ToolStart { call } => {
      let view = state.view_mut(session_id);
      view.tools.insert(call.id.clone(), call.name.clone());
      view.entries.push(Entry::new(
        EntryKind::Tool,
        format!("running {}", call.name),
      ));
    }
    Event::ToolEnd { id, error, .. } => {
      let view = state.view_mut(session_id);
      if let Some(name) = view.tools.remove(&id) {
        view.entries.push(Entry::new(
          if error {
            EntryKind::Error
          } else {
            EntryKind::Tool
          },
          format!("{} {}", name, if error { "failed" } else { "complete" }),
        ));
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
      state.view_mut(session_id).entries.push(Entry::new(
        EntryKind::System,
        format!("compacted {archived_messages} messages"),
      ));
    }
    Event::Steering { message } => state.view_mut(session_id).entries.push(Entry::new(
      EntryKind::System,
      format!("steering applied: {message}"),
    )),
    Event::Cancelled => state
      .view_mut(session_id)
      .entries
      .push(Entry::new(EntryKind::System, "run cancelled".into())),
    Event::Error { message } => state
      .view_mut(session_id)
      .entries
      .push(Entry::new(EntryKind::Error, message)),
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
      Some(Entry::new(kind, text))
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
    render_command_palette(frame, transcript, status, state, commands);
  }
  if let Some(approval) = &state.approval {
    render_approval(frame, approval);
  }
}

fn render_command_palette(
  frame: &mut Frame,
  transcript: Rect,
  above: Rect,
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
    above.y.saturating_sub(height),
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
        Span::styled(format!("  {}", command.source), Style::default().fg(MUTED)),
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
    state.splash(area.width.saturating_sub(1) as usize, area.height as usize)
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
    .unwrap_or_else(|| masthead::render(width, style));
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
      "/help commands · ctrl+1…9 select · ctrl+= / ctrl+- cycle · ctrl+l roster",
      Style::default().fg(MUTED),
    )
    .alignment(Alignment::Center),
  ]);
}

fn selected_header(preference: &str, catalog: &HeaderCatalog) -> (usize, Option<HeaderArt>) {
  let style = masthead::select_index(masthead::VARIANTS);
  match preference {
    "builtin" => (style, None),
    "random" if !catalog.headers.is_empty() => {
      let choice = masthead::select_index(masthead::VARIANTS + catalog.headers.len());
      if choice < masthead::VARIANTS {
        (choice, None)
      } else {
        (
          style,
          Some(catalog.headers[choice - masthead::VARIANTS].clone()),
        )
      }
    }
    "random" => (style, None),
    name => (style, catalog.get(name).cloned()),
  }
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
          Span::styled(format!(" {} ", entry.at), Style::default().fg(MUTED)),
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
  let permissions = permission_name(config.permissions);
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
  let model = &config.model;
  let text = if area.width >= 108 {
    format!(
      " {provider}/{model} │ {permissions} │ tokens in {input} out {output} total {total} │ \
       agents {running}/{total_agents} │ {activity} │ ^L roster "
    )
  } else if area.width >= 72 {
    format!(
      " {provider}/{model} │ {permissions} │ tok {input}↓ {output}↑ Σ{total} │ \
       ag {running}/{total_agents} │ {activity} "
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
  // long input keeps its tail in view, where the cursor is
  let capacity = area.width.saturating_sub(3) as usize;
  let skipped = state.input.chars().count().saturating_sub(capacity);
  let shown: String = state.input.chars().skip(skipped).collect();
  let width = shown.chars().count();
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(
        prefix,
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
      ),
      Span::styled(shown, Style::default().fg(INK)),
    ])),
    area,
  );
  let x = area
    .x
    .saturating_add(2)
    .saturating_add(width.min(u16::MAX as usize) as u16)
    .min(area.right().saturating_sub(1));
  frame.set_cursor_position((x, area.y));
}

// the arguments are the decision, so they are shown, clipped to what fits
fn render_approval(frame: &mut Frame, approval: &Approval) {
  let area = centered(frame.area(), frame.area().width.min(76), 11);
  let arguments: String = approval
    .call
    .arguments
    .to_string()
    .chars()
    .take(240)
    .collect();
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
      Line::styled(arguments, Style::default().fg(INK)),
      Line::raw(""),
      Line::styled("y allow   n deny", Style::default().fg(ACCENT)),
    ])
    .wrap(Wrap { trim: false })
    .block(
      Block::default()
        .title(" approval ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(YELLOW))
        .padding(Padding::horizontal(2)),
    ),
    area,
  );
}

fn clock() -> String {
  let seconds = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
    % 86_400;
  format!("{:02}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}
