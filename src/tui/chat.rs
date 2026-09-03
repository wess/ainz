use std::{
  cell::{Cell, RefCell},
  collections::BTreeMap,
  path::PathBuf,
  sync::Arc,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use ainz::{
  Agent, Approver, Config, Event, EventSink, HeaderArt, HeaderCatalog, McpProfile, PermissionMode,
  PluginCatalog, PromptCatalog, RunController, RuntimeProvider, Session, SessionStore,
  SkillCatalog,
  command_palette::{SlashCommand, builtins as builtin_commands, matches as command_matches},
  protocol::{Image, ToolCall, Usage},
  run_control,
  tool::Risk,
};
use anyhow::{Context, Result};
use crossterm::event::{
  Event as InputEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
};
use futures_util::StreamExt;
use ratatui::{
  Frame,
  layout::{Alignment, Constraint, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::Widget,
  widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
};
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};
use uuid::Uuid;

use super::{
  ACCENT, ACTIVE, BLUE, CYAN, INK, MAGENTA, MUTED, RED, Term, YELLOW, centered,
  enter_inline_terminal, enter_terminal, input, input::Input, leave_terminal, masthead,
};
use crate::app::{expand_prompt, make_agent_with};

type RunOutput = (Agent<RuntimeProvider>, Session, Result<String>);
type RunTask = JoinHandle<RunOutput>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatNext {
  Quit,
  Configure,
  Settings,
  Import,
}

pub(crate) struct ChatOutcome {
  pub session: Session,
  pub next: ChatNext,
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
  // nothing happened; the run is still going and the clock on it needs redrawing
  Tick,
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
  // the whole of what a tool returned, kept for ctrl+o rather than shown by default
  detail: Option<String>,
  at: String,
}

impl Entry {
  fn new(kind: EntryKind, text: String) -> Self {
    Self {
      kind,
      text,
      detail: None,
      at: clock(),
    }
  }

  fn with_detail(kind: EntryKind, text: String, detail: String) -> Self {
    Self {
      detail: (!detail.trim().is_empty()).then_some(detail),
      ..Self::new(kind, text)
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
  // a running tool call: where its entry is, and the label the entry started with
  live: BTreeMap<String, (usize, String)>,
  // the guardian this subagent was named for; empty for the primary transcript
  name: String,
  entries: Vec<Entry>,
  tools: BTreeMap<String, String>,
  assistant: Option<usize>,
  usage: Usage,
}

impl AgentView {
  fn new(state: AgentState) -> Self {
    Self {
      state,
      live: BTreeMap::new(),
      name: String::new(),
      entries: Vec::new(),
      tools: BTreeMap::new(),
      assistant: None,
      usage: Usage::default(),
    }
  }
}

struct ChatState {
  input: Input,
  primary: AgentView,
  agents: BTreeMap<String, AgentView>,
  active: Option<String>,
  roster: bool,
  scroll: u16,
  // when the running turn started, so a quiet provider still shows it is alive; None when idle
  started: Option<Instant>,
  // set by the first ctrl+c of a run; the second abandons the run
  cancelled: bool,
  approval: Option<Approval>,
  command_selected: usize,
  splash_style: usize,
  custom_header: Option<HeaderArt>,
  // the splash is thousands of cell puts that depend only on size and choice; paint it once
  splash_cache: RefCell<Option<(usize, usize, Vec<Line<'static>>)>>,
  // how far back the transcript actually reaches, and where the prompt and its menu were
  // drawn, all learned while drawing them
  reach: Cell<u16>,
  prompt_area: Cell<Rect>,
  palette_area: Cell<Rect>,
  // ctrl+o: show what tools returned in full rather than the first few lines
  expanded: bool,
  // esc counts toward a rewind only while it is the key being pressed twice
  rewinding: bool,
  // vim's `d`, waiting for what to delete
  pending_delete: bool,
  files: Vec<String>,
  // how much of the transcript has already been handed to the terminal's scrollback
  flushed: usize,
  inline: bool,
}

impl Default for ChatState {
  fn default() -> Self {
    Self {
      input: Input::default(),
      primary: AgentView::new(AgentState::Running),
      agents: BTreeMap::new(),
      active: None,
      roster: true,
      scroll: 0,
      started: None,
      cancelled: false,
      approval: None,
      command_selected: 0,
      splash_style: 0,
      custom_header: None,
      splash_cache: RefCell::new(None),
      reach: Cell::new(0),
      prompt_area: Cell::new(Rect::ZERO),
      palette_area: Cell::new(Rect::ZERO),
      expanded: false,
      rewinding: false,
      pending_delete: false,
      files: Vec::new(),
      flushed: 0,
      inline: false,
    }
  }
}

impl ChatState {
  fn busy(&self) -> bool {
    self.started.is_some()
  }

  // scrolling stops where the transcript does, so coming back down takes as long as going up did
  fn scroll_back(&mut self, lines: u16) {
    self.scroll = self.scroll.saturating_add(lines).min(self.reach.get());
  }

  fn scroll_forward(&mut self, lines: u16) {
    self.scroll = self.scroll.saturating_sub(lines);
  }

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
    let Some(id) = self.active.as_ref() else {
      return "#main".into();
    };
    match self.agents.get(id) {
      Some(view) if !view.name.is_empty() => format!("#{}", view.name),
      _ => format!("#{}", id.chars().take(8).collect::<String>()),
    }
  }
}

pub(crate) async fn run_chat(
  workspace: PathBuf,
  config: &mut Config,
  initial_session: Session,
  initial_prompt: Option<String>,
) -> Result<ChatOutcome> {
  // inline keeps the terminal's own scrollback; the full screen keeps the roster beside the talk
  let mut notice = None;
  let (mut terminal, inline) = match config.ui.inline {
    // the inline viewport has to ask the terminal where the cursor is; not every terminal, pipe
    // or multiplexer answers, and a session that cannot start is worse than one drawn the old way
    true => match enter_inline_terminal(INLINE_ROWS) {
      Ok(terminal) => (terminal, true),
      Err(error) => {
        notice = Some(format!(
          "inline drawing is not available here ({error}); using the full screen"
        ));
        (enter_terminal()?, false)
      }
    },
    false => (enter_terminal()?, false),
  };
  let result = run_chat_inner(
    &mut terminal,
    workspace,
    config,
    initial_session,
    initial_prompt,
    inline,
    notice,
  )
  .await;
  leave_terminal(&mut terminal)?;
  result
}

/// Status, prompt, and room for the reply being written, when drawing inline.
const INLINE_ROWS: u16 = 8;

/// Moves finished transcript into the terminal's scrollback, where its own scroll can reach it.
fn flush_scrollback(terminal: &mut Term, state: &mut ChatState) -> Result<()> {
  let width = terminal.size()?.width.max(1);
  // whatever is still being written stays in the viewport until it is done
  let live = match state.primary.assistant {
    Some(index) if state.busy() => index,
    _ => state.primary.entries.len(),
  };
  state.flushed = state.flushed.min(state.primary.entries.len());
  while state.flushed < live {
    let lines = entry_lines(&state.primary.entries[state.flushed], state.expanded);
    let height: u16 = lines
      .iter()
      .map(|line| (line.width().max(1) as u16).div_ceil(width))
      .sum();
    terminal.insert_before(height.max(1), |buffer| {
      Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(buffer.area, buffer);
    })?;
    state.flushed += 1;
  }
  Ok(())
}

async fn run_chat_inner(
  terminal: &mut Term,
  workspace: PathBuf,
  config: &mut Config,
  initial_session: Session,
  initial_prompt: Option<String>,
  inline: bool,
  notice: Option<String>,
) -> Result<ChatOutcome> {
  let (tx, mut rx) = mpsc::unbounded_channel();
  let events_tx = tx.clone();
  let events = EventSink::new(move |event| drop(events_tx.send(UiEvent::Agent(event))));
  let approval_tx = tx;
  // the rules the session is running under, shared so that allowing something always takes
  // effect for the rest of this run rather than the next one
  let rules = Arc::new(std::sync::Mutex::new(config.rules.clone()));
  let live = rules.clone();
  let approver: Approver = Arc::new(move |call, risk| {
    if live.lock().is_ok_and(|rules| {
      rules.decide(&call.name, ainz::agent::subject(&call.arguments)) == Some(true)
    }) {
      return Box::pin(async { true });
    }
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
    let usage = match prompt.hint.as_deref() {
      Some(hint) if !hint.is_empty() => format!("/{} {hint}", prompt.name),
      _ => format!("/{} [ARGS]", prompt.name),
    };
    commands.push(SlashCommand::new(
      &prompt.name,
      usage,
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
  // what was asked before in this session, so the walk back through prompts survives a resume
  let history = session
    .as_ref()
    .map(|session| {
      session
        .nodes
        .iter()
        .filter(|node| node.message.role == ainz::protocol::Role::User)
        .filter_map(|node| node.message.content.clone())
        .collect()
    })
    .unwrap_or_default();
  let files = {
    let root = workspace.clone();
    tokio::task::spawn_blocking(move || workspace_files(&root))
      .await
      .unwrap_or_default()
  };
  let mut state = ChatState {
    input: Input::with_history(history),
    inline,
    files,
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
  if let Some(notice) = notice {
    state
      .primary
      .entries
      .push(Entry::new(EntryKind::System, notice));
  }
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
    state.started = Some(Instant::now());
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
    if state.inline {
      flush_scrollback(terminal, &mut state)?;
    }
    terminal.draw(|frame| render(frame, &state, config, current_id, &workspace, &commands))?;
    let wake = tokio::select! {
      message = rx.recv() => Wake::Agent(message),
      joined = join(task.as_mut()), if task.is_some() => Wake::Finished(Box::new(joined)),
      event = input.next() => Wake::Input(event),
      () = tokio::time::sleep(Duration::from_secs(1)), if state.busy() => Wake::Tick,
    };
    let key = match wake {
      Wake::Agent(Some(message)) => {
        apply_event(&mut state, message);
        continue;
      }
      Wake::Agent(None) | Wake::Tick => continue,
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
        state.started = None;
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
          state.input.insert_str(&super::flatten_paste(&text));
          state.command_selected = 0;
        }
        continue;
      }
      Wake::Input(Some(Ok(InputEvent::Mouse(mouse)))) => {
        match mouse.kind {
          MouseEventKind::ScrollUp => state.scroll_back(3),
          MouseEventKind::ScrollDown => state.scroll_forward(3),
          // a click lands either in the prompt, where it moves the cursor, or on a suggestion
          MouseEventKind::Down(_) => {
            let palette = state.palette_area.get();
            let prompt = state.prompt_area.get();
            // the row under the pointer, past the menu's own border line
            if let Some(row) = mouse
              .row
              .checked_sub(palette.y + 1)
              .filter(|_| contains(palette, mouse.column, mouse.row))
            {
              state.command_selected = usize::from(row);
              accept_command(&mut state, &commands);
            } else if contains(prompt, mouse.column, mouse.row) {
              state.input.place(
                usize::from(mouse.row - prompt.y),
                usize::from(mouse.column.saturating_sub(prompt.x + 2)),
              );
            }
          }
          _ => {}
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
        // the same decision, kept: it applies to the rest of this run and every one after
        KeyCode::Char('a') | KeyCode::Char('A') => {
          let rule = approval_rule(&approval);
          let _ = approval.reply.send(true);
          if let Ok(mut live) = rules.lock() {
            live.allow.push(rule.clone());
          }
          config.rules.allow.push(rule.clone());
          config.rules.allow.sort();
          config.rules.allow.dedup();
          config.save().await?;
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            format!("always allowing {rule}; /permissions rules lists them"),
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
        // the line editing every other prompt on this machine already answers to
        KeyCode::Char('a') => state.input.home(),
        KeyCode::Char('e') => state.input.end(),
        KeyCode::Char('u') => state.input.kill_to_start(),
        KeyCode::Char('k') => state.input.kill_to_end(),
        KeyCode::Char('w') | KeyCode::Backspace => {
          state.input.delete_word();
          state.command_selected = 0;
        }
        KeyCode::Left => state.input.word_left(),
        KeyCode::Right => state.input.word_right(),
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
        KeyCode::Char('o') => state.expanded = !state.expanded,
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
              next: ChatNext::Quit,
            });
          }
          None => {
            return Ok(ChatOutcome {
              session: session.context("session unavailable")?,
              next: ChatNext::Quit,
            });
          }
        },
        _ => {}
      }
      continue;
    }
    let suggestion_count = suggestions(&state, &commands).len();
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
        KeyCode::Enter
          if !command_is_exact(state.input.as_str(), &commands)
            && !state.input.as_str().ends_with(' ') =>
        {
          accept_command(&mut state, &commands);
          continue;
        }
        _ => {}
      }
    }
    if key.code != KeyCode::Esc {
      state.rewinding = false;
    }
    // vim's normal mode answers first, and only for the keys it claims
    if config.ui.vim && state.input.mode() == input::Mode::Normal && vim_key(&mut state, key.code) {
      continue;
    }
    // control keys are answered above; alt is the other word-wise modifier
    let word = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
      KeyCode::Char(ch) if state.active.is_none() => {
        state.input.insert(ch);
        state.command_selected = 0;
      }
      KeyCode::Backspace if word => {
        state.input.delete_word();
        state.command_selected = 0;
      }
      KeyCode::Backspace => {
        state.input.backspace();
        state.command_selected = 0;
      }
      KeyCode::Delete => {
        state.input.delete();
        state.command_selected = 0;
      }
      KeyCode::Left if word => state.input.word_left(),
      KeyCode::Left => state.input.left(),
      KeyCode::Right if word => state.input.word_right(),
      KeyCode::Right => state.input.right(),
      KeyCode::Home => state.input.home(),
      KeyCode::End => state.input.end(),
      // the transcript is on the alternate screen, so it needs its own scrollback
      KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => state.scroll_back(1),
      KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => state.scroll_forward(1),
      KeyCode::Up => {
        state.input.previous();
        state.command_selected = 0;
      }
      KeyCode::Down => {
        state.input.next();
        state.command_selected = 0;
      }
      KeyCode::PageUp => state.scroll_back(8),
      KeyCode::PageDown => state.scroll_forward(8),
      KeyCode::Esc if state.busy() => {
        if let Some(controller) = &controller {
          controller.cancel();
        }
      }
      // esc twice steps back to the last prompt, which lands in the line ready to be changed
      KeyCode::Esc if state.rewinding => {
        state.rewinding = false;
        let entry = match session.as_mut().and_then(rewind) {
          Some(text) => {
            state.input.set(text);
            state.primary.entries = session_entries(session.as_ref().expect("rewound session"));
            state.primary.assistant = None;
            // the scrollback already holds what was said; only what comes next is new
            state.flushed = state.primary.entries.len();
            Entry::new(
              EntryKind::System,
              "rewound; edit the prompt and send it to take the session from there".into(),
            )
          }
          None => Entry::new(EntryKind::System, "no earlier prompt to rewind to".into()),
        };
        state.primary.entries.push(entry);
      }
      KeyCode::Esc => {
        state.rewinding = true;
        if config.ui.vim {
          state.input.set_mode(input::Mode::Normal);
        }
      }
      // a newline where the terminal can say so, and a trailing backslash where it cannot
      KeyCode::Enter
        if state.active.is_none()
          && (key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
            || state.input.as_str().ends_with('\\')) =>
      {
        if state.input.as_str().ends_with('\\') {
          state.input.backspace();
        }
        state.input.insert('\n');
      }
      KeyCode::Enter if state.active.is_none() && !state.input.as_str().trim().is_empty() => {
        let input = state.input.submit();
        if state.busy() {
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
              next: ChatNext::Quit,
            });
          }
          CommandResult::Configure => {
            return Ok(ChatOutcome {
              session: session.take().unwrap(),
              next: ChatNext::Configure,
            });
          }
          CommandResult::Settings => {
            return Ok(ChatOutcome {
              session: session.take().unwrap(),
              next: ChatNext::Settings,
            });
          }
          CommandResult::Import => {
            return Ok(ChatOutcome {
              session: session.take().unwrap(),
              next: ChatNext::Import,
            });
          }
          CommandResult::Recall(query) => {
            let entry = match memory_for(&workspace, config).await {
              Err(entry) => entry,
              Ok(store) => match store.recall(&query, 8).await {
                Ok(records) if records.is_empty() => {
                  Entry::new(EntryKind::System, "no memories matched".into())
                }
                Ok(records) => Entry::new(
                  EntryKind::System,
                  records
                    .iter()
                    .map(|record| format!("{}  {}", record.id, record.summary(96)))
                    .collect::<Vec<_>>()
                    .join("\n"),
                ),
                Err(error) => Entry::new(EntryKind::Error, format!("{error:#}")),
              },
            };
            state.primary.entries.push(entry);
            continue;
          }
          CommandResult::Remember(content) => {
            let entry = match memory_for(&workspace, config).await {
              Err(entry) => entry,
              Ok(store) => match store
                .remember(&content, Some("typed in a session"), "project", &[])
                .await
              {
                Ok(message) => Entry::new(EntryKind::System, message),
                Err(error) => Entry::new(EntryKind::Error, format!("{error:#}")),
              },
            };
            state.primary.entries.push(entry);
            continue;
          }
          CommandResult::ShowAgents => {
            set_roster(&mut state, config, true).await;
            continue;
          }
          CommandResult::Yeet => {
            config.permissions = PermissionMode::Auto;
            config.yeet = true;
            let (new_agent, new_options) =
              make_agent_with(&workspace, config, events.clone(), approver.clone()).await?;
            agent = Some(new_agent);
            options = new_options;
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              "yeet: for the rest of this session every tool call runs without asking, \
               and unapproved plugins load"
                .into(),
            ));
            continue;
          }
          CommandResult::SetPermissions(mode) => {
            config.permissions = mode;
            // choosing a mode is taking the wheel back, so it also ends yeet's plugin trust
            config.yeet = false;
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
          CommandResult::SetVim(on) => {
            config.ui.vim = on;
            config.save().await?;
            state.input.set_mode(input::Mode::Insert);
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              format!(
                "vim keys {}",
                if on { "on; esc for normal mode" } else { "off" }
              ),
            ));
            continue;
          }
          CommandResult::ClearRules => {
            config.rules.allow.clear();
            config.rules.deny.clear();
            if let Ok(mut live) = rules.lock() {
              live.allow.clear();
              live.deny.clear();
            }
            config.save().await?;
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              "standing rules cleared; every call asks again".into(),
            ));
            continue;
          }
          CommandResult::SetInline(on) => {
            config.ui.inline = on;
            config.save().await?;
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              format!(
                "inline {}; it takes effect next launch",
                if on {
                  "on: the terminal keeps its own scrollback"
                } else {
                  "off: full screen with the roster"
                }
              ),
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
            state.started = Some(Instant::now());
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

// the transcript is the place to say a memory command cannot run, not the process exit code
async fn memory_for(
  workspace: &std::path::Path,
  config: &Config,
) -> std::result::Result<ainz::MemoryStore, Entry> {
  match crate::app::memory_store(workspace, config).await {
    Ok(store) if store.is_off() => Err(Entry::new(
      EntryKind::System,
      "memory is off; turn it on in /settings".into(),
    )),
    Ok(store) => Ok(store),
    Err(error) => Err(Entry::new(EntryKind::Error, format!("{error:#}"))),
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
  Settings,
  Import,
  Yeet,
  Recall(String),
  Remember(String),
  ShowAgents,
  SetPermissions(PermissionMode),
  SetHeader(String),
  SetVim(bool),
  SetInline(bool),
  ClearRules,
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
    "/settings" => Ok(CommandResult::Settings),
    "/import" | "/mcp import" => Ok(CommandResult::Import),
    "/memory" => Ok(CommandResult::Recall(String::new())),
    "/synapse" => {
      let installed = ainz::synapse::binary(&config.synapse)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| format!("not installed — {}", ainz::synapse::SITE));
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        format!(
          "synapse   {}\nbinary    {installed}\nmesh      {}\nmemory    {}\nchange these in /settings",
          if config.synapse.enabled { "on" } else { "off" },
          if config.synapse.mesh { "on" } else { "off" },
          config.memory.backend.label(),
        ),
      ));
      Ok(CommandResult::Handled)
    }
    "/agents" => Ok(CommandResult::ShowAgents),
    "/vim" => Ok(CommandResult::SetVim(!config.ui.vim)),
    "/permissions rules" | "/rules" => {
      let listing = match (config.rules.allow.is_empty(), config.rules.deny.is_empty()) {
        (true, true) => "no standing rules; press a at a permission prompt to add one".into(),
        _ => {
          let allow = config
            .rules
            .allow
            .iter()
            .map(|rule| format!("allow  {rule}"))
            .collect::<Vec<_>>();
          let deny = config
            .rules
            .deny
            .iter()
            .map(|rule| format!("deny   {rule}"))
            .collect::<Vec<_>>();
          [allow, deny].concat().join("\n")
        }
      };
      state
        .primary
        .entries
        .push(Entry::new(EntryKind::System, listing));
      Ok(CommandResult::Handled)
    }
    "/rules clear" | "/permissions rules clear" => Ok(CommandResult::ClearRules),
    "/inline" => Ok(CommandResult::SetInline(!config.ui.inline)),
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
        format!(
          "permissions: {}{}",
          permission_name(config.permissions),
          if config.yeet { " (yeet)" } else { "" }
        ),
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
          "{listing}\n\n\
           up/down    earlier prompts        shift+enter  newline\n\
           @name      complete a path        esc esc      rewind a prompt\n\
           ctrl+o     expand tool output     /vim /inline prompt and drawing\n\
           ctrl+a/e   line start/end         ctrl+w       delete word\n\
           ctrl+u/k   clear before/after     alt+←/→      move a word\n\
           wheel      scroll the transcript  page up/down  scroll a screen\n\
           shift+↑/↓  scroll a line          esc          cancel a run\n\
           ctrl+1…9   select agent           ctrl+= / ctrl+-  cycle agents\n\
           ctrl+l     roster                 ctrl+c       quit"
        ),
      ));
      Ok(CommandResult::Handled)
    }
    "/new" | "/clear" => {
      *session = Session::new(session.workspace.clone());
      state.primary.entries.clear();
      state.flushed = 0;
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
    _ if input.starts_with("/memory ") => Ok(CommandResult::Recall(input[8..].trim().into())),
    _ if input.starts_with("/remember ") => {
      let content = input[10..].trim();
      if content.is_empty() {
        anyhow::bail!("usage: /remember TEXT");
      }
      Ok(CommandResult::Remember(content.into()))
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
    "/yeet" => Ok(CommandResult::Yeet),
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
  let matches = suggestions(state, commands);
  match matches.get(state.command_selected.min(matches.len().saturating_sub(1))) {
    Some(Suggestion::Command(command)) => state.input.set(command.completion()),
    Some(Suggestion::Path(path)) => {
      if let Some((at, fragment)) = file_fragment(state.input.as_str(), state.input.cursor()) {
        let mut text = state.input.as_str().to_string();
        text.replace_range(at..at + 1 + fragment.len(), &format!("@{path} "));
        state.input.set(text);
      }
    }
    None => {}
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
      let label = tool_label(&call.name, &call.arguments);
      view
        .entries
        .push(Entry::new(EntryKind::Tool, label.clone()));
      view
        .live
        .insert(call.id.clone(), (view.entries.len() - 1, label));
    }
    // the last line a running tool wrote, under the call that is writing it
    Event::ToolDelta { id, text } => {
      let view = state.view_mut(session_id);
      let Some((index, label)) = view.live.get(&id).cloned() else {
        return;
      };
      let Some(entry) = view.entries.get_mut(index) else {
        return;
      };
      entry.detail.get_or_insert_with(String::new).push_str(&text);
      let last = entry
        .detail
        .as_deref()
        .unwrap_or_default()
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_string();
      entry.text = match last.is_empty() {
        true => label,
        false => format!("{label}\n  {}", clip(&last, 160)),
      };
    }
    Event::ToolEnd { id, output, error } => {
      let view = state.view_mut(session_id);
      // the live line goes when the result arrives; the label it belonged to stays
      if let Some((index, label)) = view.live.remove(&id)
        && let Some(entry) = view.entries.get_mut(index)
      {
        entry.text = label;
      }
      if let Some(name) = view.tools.remove(&id) {
        view.entries.push(Entry::with_detail(
          if error {
            EntryKind::Error
          } else {
            EntryKind::Tool
          },
          tool_result(&name, &output, error),
          output,
        ));
      }
    }
    Event::SubagentStart {
      session_id, name, ..
    } => {
      let view = state
        .agents
        .entry(session_id)
        .or_insert_with(|| AgentView::new(AgentState::Running));
      view.state = AgentState::Running;
      view.name = name;
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

// the conversation as it stands, which after a rewind is not every node ever written
fn active_path(session: &Session) -> Vec<&ainz::session::SessionNode> {
  let mut path = Vec::new();
  let mut current = session.cursor;
  while let Some(id) = current {
    let Some(node) = session.nodes.iter().find(|node| node.id == id) else {
      break;
    };
    path.push(node);
    current = node.parent;
  }
  path.reverse();
  path
}

/// Steps the session back to before its last prompt, and hands that prompt back to be edited.
fn rewind(session: &mut Session) -> Option<String> {
  let path = active_path(session);
  let node = path
    .iter()
    .rev()
    .find(|node| node.message.role == ainz::protocol::Role::User)?;
  let (text, parent) = (node.message.content.clone()?, node.parent);
  session.checkout(parent).ok()?;
  Some(text)
}

fn session_entries(session: &Session) -> Vec<Entry> {
  active_path(session)
    .into_iter()
    .filter_map(|node| {
      let text = node.message.content.clone()?;
      let kind = match node.message.role {
        ainz::protocol::Role::User => EntryKind::User,
        ainz::protocol::Role::Assistant => EntryKind::Assistant,
        ainz::protocol::Role::System => EntryKind::System,
        ainz::protocol::Role::Tool => EntryKind::Tool,
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
  // the prompt grows with what is written in it, up to a few lines
  let prompt_rows = (state.input.as_str().matches('\n').count() + 1).clamp(1, 6) as u16;
  if state.inline {
    let [body, status, input] = Layout::vertical([
      Constraint::Min(1),
      Constraint::Length(1),
      Constraint::Length(prompt_rows),
    ])
    .areas(frame.area());
    render_transcript(frame, body, state);
    render_status(frame, status, state, config);
    render_input(frame, input, state);
    render_command_palette(frame, body, status, state, commands);
    if let Some(approval) = &state.approval {
      render_approval(frame, approval);
    }
    return;
  }
  let [header, body, status, input] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Min(8),
    Constraint::Length(1),
    Constraint::Length(prompt_rows),
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
  let matches = suggestions(state, commands);
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
  state.palette_area.set(area);
  let selected = state.command_selected.min(matches.len() - 1);
  let visible = height.saturating_sub(2) as usize;
  let start = selected.saturating_sub(visible.saturating_sub(1));
  let items = matches
    .iter()
    .enumerate()
    .skip(start)
    .take(visible)
    .map(|(index, suggestion)| {
      let (usage, description, source) = match suggestion {
        Suggestion::Command(command) => (
          command.usage.clone(),
          command.description.clone(),
          command.source.clone(),
        ),
        Suggestion::Path(path) => (
          format!("@{}", path.rsplit('/').next().unwrap_or(path)),
          path.clone(),
          "file".into(),
        ),
      };
      ListItem::new(Line::from(vec![
        Span::styled(
          format!(" {usage:<24}"),
          Style::default().fg(if index == selected {
            Color::White
          } else {
            CYAN
          }),
        ),
        Span::styled(
          description,
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
        " Ainz",
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
  let view = state.active_view();
  let entries = if state.inline && state.active.is_none() {
    &view.entries[state.flushed.min(view.entries.len())..]
  } else {
    &view.entries[..]
  };
  let lines = if entries.is_empty() && !state.inline {
    state.splash(area.width.saturating_sub(1) as usize, area.height as usize)
  } else if entries.is_empty() {
    Vec::new()
  } else {
    entries
      .iter()
      .flat_map(|entry| entry_lines(entry, state.expanded))
      .collect()
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
  state.reach.set(bottom.min(u16::MAX as usize) as u16);
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

fn entry_lines(entry: &Entry, expanded: bool) -> Vec<Line<'static>> {
  let (nick, color) = match entry.kind {
    EntryKind::User => ("you", CYAN),
    EntryKind::Assistant => ("Ainz", ACTIVE),
    EntryKind::System => ("*", YELLOW),
    EntryKind::Tool => ("tool", MAGENTA),
    EntryKind::Error => ("error", RED),
  };
  let body = match (&entry.detail, expanded) {
    (Some(detail), true) => detail.as_str(),
    _ => entry.text.as_str(),
  };
  body
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
    let label = if view.name.is_empty() {
      id.chars().take(10).collect::<String>()
    } else {
      view.name.clone()
    };
    ListItem::new(Line::from(vec![
      Span::styled(format!("{key} {marker} "), Style::default().fg(color)),
      Span::styled(label, Style::default().fg(color)),
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
  let permissions = if config.yeet {
    "yeet"
  } else {
    permission_name(config.permissions)
  };
  let mut activity = view
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
      } else if state.busy() {
        "thinking".into()
      } else {
        "ready".into()
      }
    });
  // a headless coding agent can work for minutes without a word; the clock says it has not wedged
  if let Some(seconds) = state.started.map(|start| start.elapsed().as_secs())
    && seconds > 0
  {
    activity = format!("{activity} {}", elapsed(seconds));
  }
  let model = &config.model;
  // what memory this session has, since it changes what the model can be expected to know
  let memory = match (config.memory.backend, config.mesh_active()) {
    (ainz::MemoryBackend::Off, _) => String::new(),
    (backend, true) => format!(" │ {} +mesh", backend.label()),
    (backend, false) => format!(" │ {}", backend.label()),
  };
  let text = if area.width >= 108 {
    format!(
      " {provider}/{model} │ {permissions}{memory} │ tokens in {input} out {output} total \
       {total} │ agents {running}/{total_agents} │ {activity} │ ^L roster "
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

// what the call is actually doing, the way the coding agents put it: shell(git status).
// the key order is what these tools name their subject, whichever harness sent the call
fn approval_rule(approval: &Approval) -> String {
  ainz::PermissionRules::rule_for(
    &approval.call.name,
    ainz::agent::subject(&approval.call.arguments),
  )
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
  area.width > 0
    && area.height > 0
    && (area.x..area.right()).contains(&column)
    && (area.y..area.bottom()).contains(&row)
}

/// What the prompt is offering to complete: a slash command, or a file under the workspace.
enum Suggestion {
  Command(SlashCommand),
  Path(String),
}

// the `@name` being typed, as a byte offset and the text after it, when the cursor is inside one
fn file_fragment(text: &str, cursor: usize) -> Option<(usize, String)> {
  let head = &text[..cursor];
  let at = head.rfind('@')?;
  if !head[..at]
    .chars()
    .next_back()
    .is_none_or(char::is_whitespace)
  {
    return None;
  }
  let fragment = &head[at + 1..];
  fragment
    .chars()
    .all(|ch| !ch.is_whitespace())
    .then(|| (at, fragment.to_string()))
}

fn suggestions(state: &ChatState, commands: &[SlashCommand]) -> Vec<Suggestion> {
  if let Some((_, fragment)) = file_fragment(state.input.as_str(), state.input.cursor()) {
    let needle = fragment.to_ascii_lowercase();
    let mut matched: Vec<_> = state
      .files
      .iter()
      .filter(|path| path.to_ascii_lowercase().contains(&needle))
      .collect();
    // a name that starts the way you typed comes before one that merely contains it
    matched.sort_by_key(|path| {
      let lower = path.to_ascii_lowercase();
      let leading = !lower
        .rsplit('/')
        .next()
        .unwrap_or(lower.as_str())
        .starts_with(&needle);
      (leading, path.len())
    });
    return matched
      .into_iter()
      .take(20)
      .map(|path| Suggestion::Path(path.clone()))
      .collect();
  }
  command_matches(commands, state.input.as_str())
    .into_iter()
    .cloned()
    .map(Suggestion::Command)
    .collect()
}

/// Walks the workspace once, so completing a path never touches the disk mid-keystroke.
fn workspace_files(root: &std::path::Path) -> Vec<String> {
  const SKIP: [&str; 6] = [
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    "__pycache__",
  ];
  let mut found = Vec::new();
  let mut queue = vec![(root.to_path_buf(), 0_usize)];
  while let Some((directory, depth)) = queue.pop() {
    if depth > 8 || found.len() >= 5000 {
      continue;
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
      continue;
    };
    for entry in entries.flatten() {
      let name = entry.file_name().to_string_lossy().to_string();
      if name.starts_with('.') || SKIP.contains(&name.as_str()) {
        continue;
      }
      let path = entry.path();
      if path.is_dir() {
        queue.push((path, depth + 1));
      } else if let Ok(relative) = path.strip_prefix(root) {
        found.push(relative.to_string_lossy().to_string());
      }
    }
  }
  found.sort();
  found
}

/// vim's normal mode, enough of it to edit a prompt. Returns whether the key was claimed.
fn vim_key(state: &mut ChatState, code: KeyCode) -> bool {
  // a pending `d` waits for what to delete
  if state.pending_delete {
    state.pending_delete = false;
    match code {
      KeyCode::Char('d') => state.input.clear(),
      KeyCode::Char('w') => state.input.delete_word_forward(),
      KeyCode::Char('b') => state.input.delete_word(),
      KeyCode::Char('$') => state.input.kill_to_end(),
      KeyCode::Char('0') => state.input.kill_to_start(),
      _ => {}
    }
    return true;
  }
  match code {
    KeyCode::Char('h') => state.input.left(),
    KeyCode::Char('l') => state.input.right(),
    KeyCode::Char('w') | KeyCode::Char('e') => state.input.word_right(),
    KeyCode::Char('b') => state.input.word_left(),
    KeyCode::Char('0') | KeyCode::Char('^') => state.input.home(),
    KeyCode::Char('$') => state.input.end(),
    KeyCode::Char('i') => state.input.set_mode(input::Mode::Insert),
    KeyCode::Char('a') => state.input.append(),
    KeyCode::Char('I') => {
      state.input.home();
      state.input.set_mode(input::Mode::Insert);
    }
    KeyCode::Char('A') => {
      state.input.end();
      state.input.set_mode(input::Mode::Insert);
    }
    KeyCode::Char('x') => state.input.delete(),
    KeyCode::Char('D') => state.input.kill_to_end(),
    KeyCode::Char('C') => {
      state.input.kill_to_end();
      state.input.set_mode(input::Mode::Insert);
    }
    KeyCode::Char('d') => state.pending_delete = true,
    KeyCode::Char('k') => {
      state.input.previous();
    }
    KeyCode::Char('j') => {
      state.input.next();
    }
    // enter still sends, and anything else falls through to the ordinary handling
    _ => return false,
  }
  true
}

fn tool_label(name: &str, arguments: &serde_json::Value) -> String {
  const SUBJECT: [&str; 12] = [
    "name",
    "tool",
    "command",
    "action",
    "path",
    "file_path",
    "pattern",
    "query",
    "url",
    "description",
    "task",
    "prompt",
  ];
  let subject = SUBJECT.iter().find_map(|key| {
    arguments
      .get(key)
      .and_then(serde_json::Value::as_str)
      .map(str::trim)
      .filter(|value| !value.is_empty())
  });
  match subject {
    // a path is identified by its end, so an overlong one keeps its tail rather than its root
    Some(subject) if subject.contains('/') && !subject.contains(char::is_whitespace) => {
      format!("{name}({})", clip_start(subject, 72))
    }
    Some(subject) => format!("{name}({})", clip(subject, 72)),
    None => name.to_string(),
  }
}

fn clip_start(text: &str, width: usize) -> String {
  let count = text.chars().count();
  if count <= width {
    return text.to_string();
  }
  ['…']
    .into_iter()
    .chain(text.chars().skip(count - width + 1))
    .collect()
}

/// The first few lines of what a tool actually returned, under the call that asked for it.
fn tool_result(name: &str, output: &str, error: bool) -> String {
  let mut lines = output.lines().filter(|line| !line.trim().is_empty());
  let preview: Vec<_> = lines.by_ref().take(3).map(|line| clip(line, 160)).collect();
  let rest = lines.count();
  let mut text = match (preview.first(), error) {
    (None, _) => return format!("⎿ {name} {}", if error { "failed" } else { "done" }),
    // an error says which tool failed, since the failure is the part worth reading
    (Some(first), true) => format!("⎿ {name} failed: {first}"),
    (Some(first), false) => format!("⎿ {first}"),
  };
  for line in preview.iter().skip(1) {
    text.push_str("\n  ");
    text.push_str(line);
  }
  if rest > 0 {
    text.push_str(&format!("\n  … +{rest} lines"));
  }
  text
}

fn clip(text: &str, width: usize) -> String {
  let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if flat.chars().count() <= width {
    return flat;
  }
  flat
    .chars()
    .take(width.saturating_sub(1))
    .chain(['…'])
    .collect()
}

fn elapsed(seconds: u64) -> String {
  match seconds {
    seconds if seconds < 60 => format!("{seconds}s"),
    seconds => format!("{}m{:02}s", seconds / 60, seconds % 60),
  }
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
  state.prompt_area.set(area);
  let prefix = match (state.busy(), state.input.mode()) {
    (true, _) => "↳ ",
    (false, input::Mode::Normal) => "▪ ",
    (false, input::Mode::Insert) => "> ",
  };
  let capacity = area.width.saturating_sub(3).max(1) as usize;
  let text = state.input.as_str();
  let before = &text[..state.input.cursor()];
  let row = before.matches('\n').count();
  let column = before
    .rsplit('\n')
    .next()
    .unwrap_or_default()
    .chars()
    .count();
  // a line longer than the box scrolls to keep the cursor in view
  let skipped = column.saturating_sub(capacity);
  let lines: Vec<Line> = text
    .split('\n')
    .enumerate()
    .map(|(index, line)| {
      let shown: String = line
        .chars()
        .skip(if index == row { skipped } else { 0 })
        .collect();
      Line::from(vec![
        Span::styled(
          if index == 0 { prefix } else { "  " },
          Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(shown, Style::default().fg(INK)),
      ])
    })
    .collect();
  frame.render_widget(Paragraph::new(lines), area);
  let x = area
    .x
    .saturating_add(2)
    .saturating_add((column - skipped).min(u16::MAX as usize) as u16)
    .min(area.right().saturating_sub(1));
  let y = area
    .y
    .saturating_add(row.min(u16::MAX as usize) as u16)
    .min(area.bottom().saturating_sub(1));
  frame.set_cursor_position((x, y));
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
      Line::styled(
        format!("a  always allow {}", approval_rule(approval)),
        Style::default().fg(MUTED),
      ),
      Line::styled("y allow   a always   n deny", Style::default().fg(ACCENT)),
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

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::{Entry, EntryKind, entry_lines, file_fragment, tool_label, tool_result};

  #[test]
  fn a_call_is_labelled_by_what_it_acts_on() {
    assert_eq!(
      tool_label("shell", &json!({"command": "git status --short"})),
      "shell(git status --short)"
    );
    assert_eq!(
      tool_label("Read", &json!({"file_path": "/tmp/notes.md"})),
      "Read(/tmp/notes.md)"
    );
    // nothing recognisable to name, so the tool stands alone rather than showing raw JSON
    assert_eq!(tool_label("TodoWrite", &json!({"todos": []})), "TodoWrite");
  }

  #[test]
  fn a_long_path_keeps_the_end_that_names_the_file() {
    let path = format!("/{}/notes.md", "deep".repeat(30));

    let label = tool_label("Read", &json!({ "file_path": path }));

    assert!(label.ends_with("deepdeep/notes.md)"), "{label}");
    assert!(label.starts_with("Read(…"), "{label}");
  }

  #[test]
  fn an_at_sign_starts_a_path_completion() {
    // the fragment is what has been typed after the @, wherever the cursor sits
    assert_eq!(
      file_fragment("look at @src/ma", 15),
      Some((8, "src/ma".into()))
    );
    // an address is not a path completion, since the @ has no space before it
    assert_eq!(file_fragment("mail me@wess.io", 15), None);
    // and neither is a finished word
    assert_eq!(file_fragment("@src/main.rs now", 16), None);
  }

  #[test]
  fn a_result_shows_the_first_lines_and_counts_the_rest() {
    let output = (1..=6).map(|n| format!("line {n}")).collect::<Vec<_>>();

    let text = tool_result("shell", &output.join("\n"), false);

    assert_eq!(text, "⎿ line 1\n  line 2\n  line 3\n  … +3 lines");
  }

  #[test]
  fn ctrl_o_shows_what_a_tool_returned_in_full() {
    let entry = Entry::with_detail(
      EntryKind::Tool,
      tool_result("shell", "one\ntwo\nthree\nfour", false),
      "one\ntwo\nthree\nfour".into(),
    );

    // collapsed: the three-line preview and the count of the rest
    assert_eq!(entry_lines(&entry, false).len(), 4);
    // expanded: every line the tool actually wrote
    assert_eq!(entry_lines(&entry, true).len(), 4);
    let expanded = entry_lines(&entry, true)
      .iter()
      .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
      .collect::<String>();
    assert!(expanded.contains("four"), "{expanded}");
    let collapsed = entry_lines(&entry, false)
      .iter()
      .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
      .collect::<String>();
    assert!(collapsed.contains("+1 lines"), "{collapsed}");
  }

  #[test]
  fn a_failure_names_the_tool_that_failed() {
    assert_eq!(
      tool_result("edit", "no such file", true),
      "⎿ edit failed: no such file"
    );
    assert_eq!(tool_result("write", "", false), "⎿ write done");
  }
}
