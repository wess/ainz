use std::{
  cell::{Cell, RefCell},
  collections::{BTreeMap, VecDeque},
  path::PathBuf,
  rc::Rc,
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
  release_mouse,
};
use crate::app::{expand_prompt, make_agent_with};

mod activity;
mod mcp;
mod themes;
mod title;
mod transcript;

use activity::RunStatus;
use transcript::entry_lines;

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
  // the wait for an owed frame ran out
  Tick,
  // a second passed on a running turn
  Clock,
  Redraw(Option<crate::live::Control>),
  Live(Vec<crate::live::Info>),
  Branch(Option<String>),
  Improvement(bool),
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
  Completed,
  Cancelled,
}

struct Entry {
  kind: EntryKind,
  text: String,
  stream: Option<termweave::Stream>,
  // the whole of what a tool returned, kept for ctrl+o rather than shown by default
  detail: Option<String>,
  // set for a tool call, which is drawn as a command rather than as somebody speaking
  tool: Option<ToolLine>,
  at: String,
  // bumped whenever anything above changes, so what was drawn for it can be kept
  revision: u64,
  // what this entry last laid out, and what it laid it out for. Parsing and wrapping Markdown
  // is the expensive part of drawing a buffer, and every entry but the last one or two is the
  // same on every frame of a run.
  //
  // The row count is kept for every entry, because every frame needs the height of the whole
  // buffer to know where it is scrolled to. The rows themselves are kept only while the entry
  // is on screen, so what a long session holds does not grow with it.
  rows: RefCell<Option<Rendered>>,
}

struct Rendered {
  width: usize,
  expanded: bool,
  revision: u64,
  count: usize,
  lines: Option<Rc<Vec<Line<'static>>>>,
}

impl Rendered {
  fn covers(&self, expanded: bool, width: usize, revision: u64) -> bool {
    (self.width, self.expanded, self.revision) == (width, expanded, revision)
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolState {
  Running,
  Done,
  Failed,
  Stopped,
}

impl ToolState {
  fn glyph(self) -> (&'static str, Color) {
    match self {
      Self::Running => ("▸", YELLOW),
      Self::Done => ("▪", ACTIVE),
      Self::Failed => ("✗", RED),
      Self::Stopped => ("■", YELLOW),
    }
  }
}

/// How much of one call's output the buffer keeps in memory. The whole of it is on disk in the
/// session; this is the part `ctrl+o` can put back on the screen.
const MAX_TOOL_OUTPUT: usize = 256 * 1024;

/// A call as the transcript shows it: what ran, what it ran on, how it went, and the last
/// line it wrote while it was still going.
struct ToolLine {
  name: String,
  subject: String,
  state: ToolState,
  note: String,
  live: String,
  started: Instant,
}

impl Entry {
  fn new(kind: EntryKind, text: String) -> Self {
    Self {
      kind,
      text,
      stream: None,
      detail: None,
      tool: None,
      at: clock(),
      revision: 0,
      rows: RefCell::new(None),
    }
  }

  /// Says the entry has changed, so the rows drawn for it are drawn again.
  fn touch(&mut self) {
    self.revision = self.revision.wrapping_add(1);
  }

  /// The rows this entry occupies at this width. Laid out once and kept until the entry, the
  /// width, or what `ctrl+o` is showing changes.
  fn lines(&self, expanded: bool, width: usize) -> Rc<Vec<Line<'static>>> {
    let mut cache = self.rows.borrow_mut();
    if let Some(rendered) = &*cache
      && rendered.covers(expanded, width, self.revision)
      && let Some(lines) = &rendered.lines
    {
      return lines.clone();
    }
    let lines = Rc::new(entry_lines(self, expanded, width));
    *cache = Some(Rendered {
      width,
      expanded,
      revision: self.revision,
      count: lines.len(),
      lines: Some(lines.clone()),
    });
    lines
  }

  /// How many rows this entry occupies, without keeping the rows themselves. An entry that has
  /// been drawn before answers from what it remembers.
  fn height(&self, expanded: bool, width: usize) -> usize {
    if let Some(rendered) = &*self.rows.borrow()
      && rendered.covers(expanded, width, self.revision)
    {
      return rendered.count;
    }
    let lines = entry_lines(self, expanded, width);
    let count = lines.len();
    *self.rows.borrow_mut() = Some(Rendered {
      width,
      expanded,
      revision: self.revision,
      count,
      lines: None,
    });
    count
  }

  /// Lets go of the rows while remembering how many there were, for an entry that has scrolled
  /// off the screen.
  fn release(&self) {
    if let Some(rendered) = &mut *self.rows.borrow_mut() {
      rendered.lines = None;
    }
  }

  fn call(name: String, subject: String) -> Self {
    Self {
      tool: Some(ToolLine {
        name,
        subject,
        state: ToolState::Running,
        note: String::new(),
        live: String::new(),
        started: Instant::now(),
      }),
      ..Self::new(EntryKind::Tool, String::new())
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
  topic: String,
  entries: Vec<Entry>,
  tools: BTreeMap<String, String>,
  assistant: Option<usize>,
  usage: Usage,
  // the plan as this agent's last todo call left it; each view has its own
  todos: Vec<ainz::Todo>,
}

impl AgentView {
  fn finish_message(&mut self) {
    if let Some(index) = self.assistant.take() {
      self.edit(index, |entry| entry.stream = None);
    }
  }

  /// Changes an entry and says so, so the rows drawn for it are laid out again. Every mutation
  /// of a drawn entry goes through here; one that does not would keep showing what it used to
  /// say.
  fn edit(&mut self, index: usize, change: impl FnOnce(&mut Entry)) {
    let Some(entry) = self.entries.get_mut(index) else {
      return;
    };
    change(entry);
    entry.touch();
  }

  fn new(state: AgentState) -> Self {
    Self {
      state,
      live: BTreeMap::new(),
      name: String::new(),
      topic: String::new(),
      entries: Vec::new(),
      tools: BTreeMap::new(),
      assistant: None,
      usage: Usage::default(),
      todos: Vec::new(),
    }
  }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RosterSource {
  #[default]
  Local,
  Mesh,
}

struct ChatState {
  live_hint: Option<String>,
  attention: (usize, usize, usize),
  branch: Option<String>,
  session_name: Option<String>,
  input: Input,
  primary: AgentView,
  agents: BTreeMap<String, AgentView>,
  active: Option<String>,
  roster: bool,
  roster_source: RosterSource,
  mesh_roster: Option<ainz::synapse::MeshRoster>,
  mesh_error: Option<String>,
  // where the reader is, and how far they have asked to move since the last frame; only a
  // frame knows how tall everything is, so it is the frame that settles the two
  scroll: Cell<Scroll>,
  scroll_delta: Cell<isize>,
  page: Cell<u16>,
  outcome: Option<(RunStatus, Duration)>,
  // when the running turn started, so a quiet provider still shows it is alive; None when idle
  started: Option<Instant>,
  // set by the first ctrl+c of a run; the second abandons the run
  cancelled: bool,
  approval: Option<Approval>,
  // requests that arrived while one was on screen; two subagents can both want to run
  // something, and the second must not answer the first
  waiting: VecDeque<Approval>,
  // steering accepted but not yet taken up by the run, given back if the run ends first
  steers: Vec<String>,
  command_selected: usize,
  splash_style: usize,
  custom_header: Option<HeaderArt>,
  header_preview: bool,
  theme: ainz::theme::Theme,
  art_area: Cell<Option<Rect>>,
  // the splash is thousands of cell puts that depend only on size and choice; paint it once
  splash_cache: RefCell<Option<(usize, usize, Vec<Line<'static>>)>>,
  // where the prompt and its menu were drawn, learned while drawing them
  prompt_area: Cell<Rect>,
  // the first wrapped row of the prompt on screen, once it holds more than fits
  prompt_first: Cell<usize>,
  palette_area: Cell<Rect>,
  // ctrl+o: show what tools returned in full rather than the first few lines
  expanded: bool,
  // esc counts toward a rewind only while it is the key being pressed twice
  rewinding: bool,
  // vim's `d`, waiting for what to delete
  pending_delete: bool,
  // an image pasted or dropped in, waiting for the message it belongs to
  pending_image: Option<PathBuf>,
  files: Vec<String>,
  workspace: PathBuf,
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
      roster_source: RosterSource::Local,
      mesh_roster: None,
      mesh_error: None,
      scroll: Cell::new(Scroll::Follow),
      scroll_delta: Cell::new(0),
      page: Cell::new(0),
      live_hint: None,
      attention: (0, 0, 0),
      branch: None,
      session_name: None,
      outcome: None,
      started: None,
      cancelled: false,
      approval: None,
      waiting: VecDeque::new(),
      steers: Vec::new(),
      command_selected: 0,
      splash_style: 0,
      custom_header: None,
      header_preview: false,
      theme: ainz::theme::Theme::default(),
      art_area: Cell::new(None),
      splash_cache: RefCell::new(None),
      prompt_area: Cell::new(Rect::ZERO),
      prompt_first: Cell::new(0),
      palette_area: Cell::new(Rect::ZERO),
      expanded: false,
      rewinding: false,
      pending_delete: false,
      pending_image: None,
      files: Vec::new(),
      workspace: PathBuf::new(),
      flushed: 0,
      inline: false,
    }
  }
}

impl ChatState {
  /// A path under the workspace, named by the part that is not the workspace.
  fn relative(&self, subject: &str) -> String {
    let root = self.workspace.to_string_lossy();
    match subject
      .strip_prefix(root.as_ref())
      .map(|rest| rest.trim_start_matches('/'))
    {
      Some(rest) if !rest.is_empty() => rest.to_string(),
      _ => subject.to_string(),
    }
  }

  fn busy(&self) -> bool {
    self.started.is_some()
  }

  fn scroll_back(&self, lines: u16) {
    self
      .scroll_delta
      .set(self.scroll_delta.get() - lines as isize);
  }

  fn scroll_forward(&self, lines: u16) {
    self
      .scroll_delta
      .set(self.scroll_delta.get() + lines as isize);
  }

  // a screen at a time, keeping two rows of the last one for context
  fn scroll_page(&self) -> u16 {
    self.page.get().saturating_sub(2).max(1)
  }

  fn follow(&self) {
    self.scroll.set(Scroll::Follow);
    self.scroll_delta.set(0);
  }

  fn scroll_top(&self) {
    self.scroll.set(Scroll::At { entry: 0, row: 0 });
    self.scroll_delta.set(0);
  }

  fn reading_history(&self) -> bool {
    self.scroll.get() != Scroll::Follow
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
    self.follow();
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
    self.follow();
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
  link: &mut Option<crate::live::Link>,
) -> Result<ChatOutcome> {
  // inline keeps the terminal's own scrollback; the full screen keeps the roster beside the talk
  let mut notice = None;
  let (mut terminal, inline) = match config.ui.inline && link.is_none() {
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
  if !inline {
    release_mouse(&mut terminal)?;
  }
  let result = run_chat_inner(
    &mut terminal,
    workspace,
    config,
    initial_session,
    initial_prompt,
    (inline, notice),
    link,
  )
  .await;
  leave_terminal(&mut terminal)?;
  result
}

/// Status, prompt, and room for the reply being written, when drawing inline.
const INLINE_ROWS: u16 = 8;

/// How often the screen may be repainted. Catching up at about fifty frames a second costs a
/// fraction of what painting per token does and looks the same to a reader.
const FRAME: Duration = Duration::from_millis(20);

/// How often a running turn is redrawn when nothing else happens: its clock counts seconds.
const CLOCK: Duration = Duration::from_secs(1);

/// The most queued events applied before the loop looks at the keyboard and the screen again.
const EVENT_BATCH: usize = 256;

/// Lets go of entries the terminal's scrollback already holds. Drawing inline, the terminal
/// keeps what was said; holding a second copy here would grow with the session for nothing.
/// Only between runs, while no entry is being written to by index.
fn release_flushed(state: &mut ChatState) {
  let view = &mut state.primary;
  if !state.inline || state.started.is_some() || state.flushed == 0 {
    return;
  }
  if view.assistant.is_some() || !view.live.is_empty() {
    return;
  }
  let flushed = state.flushed.min(view.entries.len());
  view.entries.drain(..flushed);
  state.flushed = 0;
  state.follow();
}

/// Moves finished transcript into the terminal's scrollback, where its own scroll can reach it.
fn flush_scrollback(terminal: &mut Term, state: &mut ChatState) -> Result<()> {
  if !state.inline {
    return Ok(());
  }
  // the same width the buffer draws at, so what leaves for the scrollback is wrapped the way
  // it was on screen rather than one column wider
  let width = terminal.size()?.width.max(1).saturating_sub(1).max(1);
  // whatever is still being written stays in the viewport until it is done
  let live = state
    .primary
    .assistant
    .into_iter()
    .chain(state.primary.live.values().map(|(index, _)| *index))
    .min()
    .unwrap_or(state.primary.entries.len());
  state.flushed = state.flushed.min(state.primary.entries.len());
  while state.flushed < live {
    let lines = entry_lines(
      &state.primary.entries[state.flushed],
      state.expanded,
      width as usize,
    );
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = paragraph.line_count(width).min(u16::MAX as usize) as u16;
    terminal.insert_before(height.max(1), |buffer| {
      paragraph.render(buffer.area, buffer);
      state.theme.apply(buffer, None);
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
  viewport: (bool, Option<String>),
  link: &mut Option<crate::live::Link>,
) -> Result<ChatOutcome> {
  let (inline, notice) = viewport;
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
  let mut command_data = CommandData {
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
  let mut live_title = initial_session.title();
  let mut live_label = initial_session.label.clone();
  let mut session = Some(initial_session);
  let session_id = session.as_ref().unwrap().id;
  let previous_prompts =
    store.list().await?.into_iter().find_map(|saved| {
      (saved.workspace == workspace && saved.id != session_id).then_some(saved.id)
    });
  let (splash_style, custom_header) = selected_header(&config.ui.header, &command_data.headers);
  // what was asked before in this session, so the walk back through prompts survives a resume
  let mut history: Vec<String> = session
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
  if history.is_empty()
    && let Some(id) = previous_prompts
    && let Ok(previous) = store.load(id).await
  {
    history = previous
      .nodes
      .iter()
      .filter(|node| node.message.role == ainz::protocol::Role::User)
      .filter_map(|node| node.message.content.clone())
      .collect();
  }
  let files = {
    let root = workspace.clone();
    tokio::task::spawn_blocking(move || workspace_files(&root))
      .await
      .unwrap_or_default()
  };
  let mut state = ChatState {
    live_hint: link.as_ref().map(|_| {
      format!(
        "  ·  {} detach  ·  {} sessions",
        ainz::keybind::label(&config.ui.keybindings, ainz::keybind::Action::Detach),
        ainz::keybind::label(&config.ui.keybindings, ainz::keybind::Action::Sessions)
      )
    }),
    input: Input::with_history(history),
    inline,
    files,
    workspace: workspace.clone(),
    primary: AgentView {
      entries: session_entries(session.as_ref().unwrap()),
      usage: session.as_ref().unwrap().usage.clone(),
      todos: session_plan(session.as_ref().unwrap()),
      ..AgentView::new(AgentState::Running)
    },
    roster: config.ui.roster_visible,
    splash_style,
    custom_header,
    ..ChatState::default()
  };
  state.primary.refresh_topic();
  themes::initialize(&mut state, config).await?;
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
    state.begin_run();
    task = Some(tokio::spawn(async move {
      let result = local_agent
        .run_controlled(&mut local_session, expanded, run_options, &mut inbox)
        .await;
      (local_agent, local_session, result)
    }));
  }

  let mut input = EventStream::new();
  // a provider writing quickly can wake this loop hundreds of times a second, and repainting
  // for each of them spends the whole budget on frames nobody can read. Deltas are applied as
  // they arrive and the screen is caught up at a rate a person can see; a keystroke never waits
  // behind that, because a key always leaves a frame owed and the wait is only what is left of
  // one.
  let mut painted = Instant::now() - FRAME;
  let mut dirty = true;
  let mut stopping = false;
  let mut resolver = ainz::keybind::Resolver::default();
  let mut monitor = crate::live::attention::monitor().await.ok();
  let mut branch = super::context::branch(&workspace).await;
  let mut improvement_watch = config
    .improve
    .enabled
    .then(super::context::improvements)
    .flatten();
  if config.improve.enabled
    && let Ok(Some(message)) =
      ainz::improve::automatic::AutomaticStore::default_store().and_then(|s| s.notice(&workspace))
  {
    state
      .primary
      .entries
      .push(Entry::new(EntryKind::System, message));
  }
  // the only thing on screen that moves by itself is the seconds count on a running turn
  let mut clock = tokio::time::interval(CLOCK);
  clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
  loop {
    // a flood of events is applied in batches, so a key pressed in the middle of one is not
    // left waiting behind the rest of it
    for _ in 0..EVENT_BATCH {
      let Ok(message) = rx.try_recv() else {
        break;
      };
      apply_event(&mut state, message);
      dirty = true;
    }
    if let Some(session) = &session {
      live_title = session.title();
      live_label = session.label.clone();
      state.session_name = Some(live_title.clone());
    }
    if let Some(link) = link.as_mut() {
      let phase = if state.approval.is_some() {
        "needs approval"
      } else if state.busy() {
        "running"
      } else if matches!(state.outcome, Some((RunStatus::Failed, _))) {
        "failed run"
      } else if state.outcome.is_some() {
        "finished run"
      } else {
        "ready"
      };
      link.state(
        current_id,
        live_title.clone(),
        live_label.clone(),
        phase,
        &config.ui,
      );
    }
    if dirty && painted.elapsed() >= FRAME {
      let drawn = flush_scrollback(terminal, &mut state).and_then(|()| {
        terminal.draw(|frame| {
          state.art_area.set(None);
          render(frame, &state, config, current_id, &workspace, &commands);
          state.theme.apply(frame.buffer_mut(), state.art_area.get());
        })?;
        Ok(())
      });
      // a terminal that will not take a frame is gone; the conversation is not, and the caller
      // saves whatever it is handed
      if drawn.is_err() {
        return Ok(ChatOutcome {
          session: match session.take() {
            Some(session) => session,
            None => store
              .load(current_id)
              .await
              .unwrap_or(Session::new(workspace.clone())),
          },
          next: ChatNext::Quit,
        });
      }
      painted = Instant::now();
      dirty = false;
      release_flushed(&mut state);
    }
    // when a frame is owed, the wait is only as long as what is left of one; otherwise an idle
    // session sleeps until something happens
    let idle = match dirty {
      true => FRAME
        .saturating_sub(painted.elapsed())
        .max(Duration::from_millis(1)),
      false => Duration::from_secs(3600),
    };
    let ticking = state.busy() && state.approval.is_none();
    let wake = tokio::select! {
      message = rx.recv() => Wake::Agent(message),
      joined = join(task.as_mut()), if task.is_some() => Wake::Finished(Box::new(joined)),
      event = input.next() => Wake::Input(event),
      redraw = async { match link.as_mut() { Some(link) => link.redraw.recv().await, None => std::future::pending().await } }, if link.is_some() => Wake::Redraw(redraw),
      infos = async { match monitor.as_mut() { Some(monitor) => monitor.updates.recv().await, None => std::future::pending().await } }, if monitor.is_some() => Wake::Live(infos.unwrap_or_default()),
      name = async { match branch.as_mut() { Some(branch) => branch.updates.recv().await, None => std::future::pending().await } }, if branch.is_some() => Wake::Branch(name.flatten()),
      changed = async { match improvement_watch.as_mut() { Some((_, updates)) => updates.recv().await, None => std::future::pending().await } }, if improvement_watch.is_some() => Wake::Improvement(changed.is_some()),
      _ = clock.tick(), if ticking => Wake::Clock,
      () = tokio::time::sleep(idle) => Wake::Tick,
    };
    let mut key = match wake {
      Wake::Improvement(open) => {
        if !open {
          improvement_watch = None;
        }
        if let Ok(Some(message)) = ainz::improve::automatic::AutomaticStore::default_store()
          .and_then(|s| s.notice(&workspace))
        {
          state
            .primary
            .entries
            .push(Entry::new(EntryKind::System, message));
          dirty = true;
        }
        continue;
      }
      Wake::Live(infos) => {
        state.attention = crate::live::attention::counts(
          &infos,
          link.as_ref().map(|link| link.id).unwrap_or(Uuid::nil()),
        );
        dirty = true;
        continue;
      }
      Wake::Branch(name) => {
        state.branch = name;
        dirty = true;
        continue;
      }
      Wake::Redraw(Some(crate::live::Control::Stop)) => {
        if let Some(controller) = &controller {
          stopping = true;
          state.cancelled = true;
          controller.cancel();
          continue;
        }
        return Ok(ChatOutcome {
          session: session.take().context("session unavailable")?,
          next: ChatNext::Quit,
        });
      }
      Wake::Redraw(request) => {
        if request.is_none() {
          anyhow::bail!("background host disconnected");
        }
        terminal.swap_buffers();
        terminal.swap_buffers();
        crossterm::execute!(
          terminal.backend_mut(),
          crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
          crossterm::event::EnableBracketedPaste
        )?;
        dirty = true;
        continue;
      }
      Wake::Agent(Some(message)) => {
        apply_event(&mut state, message);
        dirty = true;
        continue;
      }
      Wake::Clock => {
        dirty = true;
        continue;
      }
      // the wait ran out, or the sink closed; an owed frame is drawn on the way round
      Wake::Agent(None) | Wake::Tick => continue,
      Wake::Finished(joined) => {
        task = None;
        dirty = true;
        // events emitted just before the run returned are still queued; show them first
        while let Ok(message) = rx.try_recv() {
          apply_event(&mut state, message);
        }
        // a run that never returned took the agent and the session with it. The last save is
        // what is left of the conversation, and a rebuilt agent can carry on from it; losing
        // the session as well would only make it worse.
        let (returned_agent, returned_session, result) = match *joined {
          Ok(returned) => returned,
          Err(error) => {
            session = Some(match store.load(current_id).await {
              Ok(session) => session,
              Err(_) => Session::new(workspace.clone()),
            });
            controller = None;
            match make_agent_with(&workspace, config, events.clone(), approver.clone()).await {
              Ok((rebuilt, rebuilt_options)) => {
                agent = Some(rebuilt);
                options = rebuilt_options;
              }
              // nothing left to run with, but the conversation is reloaded and /config can
              // still fix whatever the provider is now refusing
              Err(rebuild) => state.primary.entries.push(Entry::new(
                EntryKind::Error,
                format!("no provider to continue with: {rebuild:#}; try /config"),
              )),
            }
            state.finish_run(Err(anyhow::anyhow!(
              "the run did not return ({error:#}); reloaded the last saved session"
            )));
            continue;
          }
        };
        agent = Some(returned_agent);
        current_id = returned_session.id;
        state.primary.usage = returned_session.usage.clone();
        // the turn is held before it is written down: a disk that will not take it is worth
        // saying so about, not worth losing the turn over
        if let Err(error) = store.save(&returned_session).await {
          state.primary.entries.push(Entry::new(
            EntryKind::Error,
            format!("could not save the session: {error:#}"),
          ));
        } else if let Some(message) =
          crate::command::improve::after_save(&returned_session, config).await
        {
          state
            .primary
            .entries
            .push(Entry::new(EntryKind::System, message));
        }
        session = Some(returned_session);
        controller = None;
        // a run long enough to walk away from says so when it is done
        if config.ui.bell
          && state
            .started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(10))
        {
          let mut out = std::io::stdout();
          drop(std::io::Write::write_all(&mut out, b"\x07"));
          drop(std::io::Write::flush(&mut out));
        }
        state.finish_run(result);
        if stopping {
          return Ok(ChatOutcome {
            session: session.take().context("session unavailable")?,
            next: ChatNext::Quit,
          });
        }
        continue;
      }
      Wake::Input(Some(Ok(InputEvent::Key(key)))) if key.kind != KeyEventKind::Release => {
        dirty = true;
        key
      }
      Wake::Input(Some(Ok(InputEvent::Paste(text)))) => {
        dirty = true;
        state.header_preview = false;
        if state.active.is_none() && state.approval.is_none() {
          // a terminal pastes an image as its path, which is a file to attach rather than
          // text to type; dragging one into the window arrives the same way
          match image_path(&text) {
            Some(path) => {
              state.primary.entries.push(Entry::new(
                EntryKind::System,
                format!("attached {}; it goes with the next message", path.display()),
              ));
              state.pending_image = Some(path);
            }
            // a paste keeps its line breaks; the prompt grows to hold them
            None => {
              let text = text.replace("\r\n", "\n").replace('\r', "\n");
              state.input.insert_str(text.trim_end_matches('\n'));
            }
          }
          state.command_selected = 0;
        }
        continue;
      }
      Wake::Input(Some(Ok(InputEvent::Mouse(mouse)))) => {
        // a pointer moving over the window changes nothing on it
        dirty |= !matches!(mouse.kind, MouseEventKind::Moved);
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
                state.prompt_first.get() + usize::from(mouse.row - prompt.y),
                usize::from(mouse.column.saturating_sub(prompt.x + 2)),
                prompt_width(prompt.width),
              );
            }
          }
          _ => {}
        }
        continue;
      }
      // the layout depends on the size; an idle session would otherwise show a garbled screen
      // until the next key
      Wake::Input(Some(Ok(InputEvent::Resize(..)))) => {
        dirty = true;
        continue;
      }
      Wake::Input(Some(Ok(_))) => continue,
      Wake::Input(Some(Err(error))) => return Err(error).context("read terminal input"),
      // the terminal went away; the conversation did not
      Wake::Input(None) => {
        return Ok(ChatOutcome {
          session: match session.take() {
            Some(session) => session,
            None => store
              .load(current_id)
              .await
              .unwrap_or(Session::new(workspace.clone())),
          },
          next: ChatNext::Quit,
        });
      }
    };
    state.header_preview = false;
    use ainz::keybind::{Action as KeyAction, Match};
    let action = resolver.resolve(&config.ui.keybindings, key, state.approval.is_some());
    match action {
      Match::Prefix => continue,
      Match::Action(action) if action.live() => {
        let action = match action {
          KeyAction::Detach => crate::live::Action::Detach,
          KeyAction::Sessions => crate::live::Action::Sessions,
          KeyAction::NewSession => crate::live::Action::New,
          KeyAction::NextSession => crate::live::Action::Next,
          KeyAction::PreviousSession => crate::live::Action::Previous,
          _ => crate::live::Action::Attention,
        };
        if let Some(link) = link.as_ref() {
          link.action(action);
        } else {
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            "Enable Background sessions in /settings, then restart; or launch ainz --detach."
              .into(),
          ));
        }
        continue;
      }
      Match::Action(KeyAction::Settings) => {
        if state.busy() {
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            "Open settings after this run finishes; /detach leaves it working.".into(),
          ));
          continue;
        }
        return Ok(ChatOutcome {
          session: session.take().context("session unavailable")?,
          next: ChatNext::Settings,
        });
      }
      Match::Action(KeyAction::Interrupt) if state.approval.is_some() => {
        if let Some(controller) = &controller {
          controller.cancel();
          state.cancelled = true;
        }
        if let Some(approval) = state.approval.take() {
          let _ = approval.reply.send(false);
        }
        state.waiting.clear();
        continue;
      }
      Match::Action(KeyAction::Quit) => {
        if !state.busy() && state.input.as_str().is_empty() {
          return Ok(ChatOutcome {
            session: session.take().context("session unavailable")?,
            next: ChatNext::Quit,
          });
        }
        continue;
      }
      Match::Action(KeyAction::Roster) => {
        let visible = !state.roster || state.roster_source != RosterSource::Local;
        state.roster_source = RosterSource::Local;
        set_roster(&mut state, config, visible).await;
        continue;
      }
      Match::Action(KeyAction::MeshRoster) => {
        state.roster_source = RosterSource::Mesh;
        set_roster(&mut state, config, true).await;
        match ainz::synapse::mesh_roster(&config.synapse).await {
          Ok(roster) => {
            state.mesh_roster = Some(roster);
            state.mesh_error = None;
          }
          Err(error) => {
            state.mesh_roster = None;
            state.mesh_error = Some(format!("{error:#}"));
          }
        }
        continue;
      }
      Match::Action(KeyAction::Permissions) => {
        config.permissions = match config.permissions {
          PermissionMode::Ask => PermissionMode::Auto,
          PermissionMode::Auto => PermissionMode::ReadOnly,
          PermissionMode::ReadOnly => PermissionMode::Ask,
        };
        config.yeet = false;
        options.permissions = config.permissions;
        remember(&mut state, config).await;
        continue;
      }
      Match::Action(action) => {
        key = ainz::keybind::parse(action.default_key())?[0];
      }
      Match::None => {
        if ainz::keybind::Action::ALL.into_iter().any(|action| {
          config.ui.keybindings.contains_key(&action)
            && ainz::keybind::parse(action.default_key())
              .is_ok_and(|keys| keys.as_slice() == [ainz::keybind::normalize(key)])
        }) {
          continue;
        }
      }
    }
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
          remember(&mut state, config).await;
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
        _ => {
          state.approval = Some(approval);
          continue;
        }
      }
      state.approval = state.waiting.pop_front();
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
        KeyCode::End => state.follow(),
        KeyCode::Home => state.scroll_top(),
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
          state.roster_source = RosterSource::Local;
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
      KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => state.scroll_top(),
      KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => state.follow(),
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
      KeyCode::PageUp => state.scroll_back(state.scroll_page()),
      KeyCode::PageDown => state.scroll_forward(state.scroll_page()),
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
            let rewound = session.as_ref().expect("rewound session");
            state.primary.entries = session_entries(rewound);
            state.primary.todos = session_plan(rewound);
            state.primary.refresh_topic();
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
        let live_action = match input.trim() {
          "/detach" => Some(crate::live::Action::Detach),
          "/sessions" => Some(crate::live::Action::Sessions),
          "/attention" => Some(crate::live::Action::Attention),
          _ => None,
        };
        if let Some(action) = live_action {
          if let Some(link) = link.as_ref() {
            link.action(action);
          } else {
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              "Launch ainz --detach, or enable Background sessions in /settings and restart."
                .into(),
            ));
          }
          continue;
        }
        if !state.busy()
          && matches!(
            input.split_whitespace().next(),
            Some("/statusline" | "/statusbar")
          )
        {
          let value = input.split_once(' ').map(|(_, value)| value.trim());
          let result: Result<()> = match value {
            None => super::status::edit(terminal, &mut config.ui.status_line).map(|_| ()),
            Some("reset" | "default") => {
              config.ui.status_line = None;
              Ok(())
            }
            Some("off" | "hide") => {
              config.ui.status_line = Some(Vec::new());
              Ok(())
            }
            Some(value) => {
              let items: Option<Vec<_>> = value
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|word| !word.is_empty())
                .map(ainz::config::StatusItem::parse)
                .collect();
              match items {
                Some(items) => {
                  config.ui.status_line = Some(items);
                  Ok(())
                }
                None => Err(anyhow::anyhow!(
                  "Unknown field; use /statusline to choose fields, /statusline reset, or /statusline off."
                )),
              }
            }
          };
          if let Err(error) = result {
            state
              .primary
              .entries
              .push(Entry::new(EntryKind::Error, error.to_string()));
          } else {
            remember(&mut state, config).await;
          }
          terminal.swap_buffers();
          terminal.swap_buffers();
          crossterm::execute!(
            terminal.backend_mut(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
          )?;
          continue;
        }
        if !state.busy() && input.trim().starts_with("/rename ") {
          let name: String = input.trim()[8..]
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
          if let Some(session) = session.as_mut() {
            session.label = (!name.is_empty()).then_some(name);
            store.save(session).await?;
          }
          continue;
        }
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
          match controller
            .as_ref()
            .map_or(Err("no active run"), |controller| {
              controller.try_steer(input.clone())
            }) {
            Ok(()) => {
              state.primary.entries.push(Entry::new(
                EntryKind::System,
                format!("steering queued: {input}"),
              ));
              state.steers.push(input);
            }
            Err(reason) => {
              state.input.set(input);
              state.primary.entries.push(Entry::new(
                EntryKind::Error,
                format!("steering not queued: {reason}; draft kept"),
              ));
            }
          }
          continue;
        }
        match themes::command(input.trim(), &mut state, config).await {
          Ok(true) => continue,
          Err(error) => {
            state
              .primary
              .entries
              .push(Entry::new(EntryKind::Error, format!("{error:#}")));
            continue;
          }
          Ok(false) => {}
        }
        match mcp::command(terminal, input.trim(), &mut state, &mut command_data).await {
          Ok(true) => continue,
          Err(error) => {
            state
              .primary
              .entries
              .push(Entry::new(EntryKind::Error, format!("{error:#}")));
            continue;
          }
          Ok(false) => {}
        }
        if input.trim() == "/headers" || input.trim().starts_with("/header ") {
          match HeaderCatalog::discover(&workspace).await {
            Ok(headers) => command_data.headers = headers,
            Err(error) => {
              state.primary.entries.push(Entry::new(
                EntryKind::Error,
                format!("reload headers: {error:#}"),
              ));
              continue;
            }
          }
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
          CommandResult::Improve(action) => {
            let text = crate::command::improve::chat(&workspace, config, &action).await;
            if config.improve.enabled && improvement_watch.is_none() {
              improvement_watch = super::context::improvements();
            }
            state.primary.entries.push(match text {
              Ok(text) => Entry::new(EntryKind::System, text),
              Err(error) => Entry::new(EntryKind::Error, format!("{error:#}")),
            });
            continue;
          }
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
              Err(entry) => *entry,
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
              Err(entry) => *entry,
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
            state.roster_source = RosterSource::Local;
            set_roster(&mut state, config, true).await;
            continue;
          }
          CommandResult::Yeet => {
            config.permissions = PermissionMode::Auto;
            config.yeet = true;
            match make_agent_with(&workspace, config, events.clone(), approver.clone()).await {
              Ok((new_agent, new_options)) => {
                agent = Some(new_agent);
                options = new_options;
              }
              Err(error) => {
                state.primary.entries.push(Entry::new(
                  EntryKind::Error,
                  format!("kept the previous setup: {error:#}"),
                ));
                continue;
              }
            }
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
            remember(&mut state, config).await;
            match make_agent_with(&workspace, config, events.clone(), approver.clone()).await {
              Ok((new_agent, new_options)) => {
                agent = Some(new_agent);
                options = new_options;
              }
              Err(error) => {
                state.primary.entries.push(Entry::new(
                  EntryKind::Error,
                  format!("kept the previous setup: {error:#}"),
                ));
                continue;
              }
            }
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              format!("permissions set to {}", permission_name(mode)),
            ));
            continue;
          }
          CommandResult::SetVim(on) => {
            config.ui.vim = on;
            remember(&mut state, config).await;
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
            remember(&mut state, config).await;
            state.primary.entries.push(Entry::new(
              EntryKind::System,
              "standing rules cleared; every call asks again".into(),
            ));
            continue;
          }
          CommandResult::SetInline(on) => {
            config.ui.inline = on;
            remember(&mut state, config).await;
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
            remember(&mut state, config).await;
            state.header_preview = !state.inline;
            if state.inline && !state.primary.entries.is_empty() {
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
            let image = image.or_else(|| state.pending_image.take());
            let expanded = match expand_prompt(&prompt, &prompts).await {
              Ok(expanded) => expanded,
              // the draft goes back in the line so it can be fixed rather than retyped
              Err(error) => {
                state.input.set(prompt);
                state
                  .primary
                  .entries
                  .push(Entry::new(EntryKind::Error, format!("{error:#}")));
                continue;
              }
            };
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
            state.begin_run();
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
) -> std::result::Result<ainz::MemoryStore, Box<Entry>> {
  match crate::app::memory_store(workspace, config).await {
    Ok(store) if store.is_off() => Err(Box::new(Entry::new(
      EntryKind::System,
      "memory is off; turn it on in /settings".into(),
    ))),
    Ok(store) => Ok(store),
    Err(error) => Err(Box::new(Entry::new(EntryKind::Error, format!("{error:#}")))),
  }
}

async fn set_roster(state: &mut ChatState, config: &mut Config, visible: bool) {
  state.roster = visible;
  config.ui.roster_visible = visible;
  remember(state, config).await;
}

/// Writes a setting down. A config file that will not take a change is worth reporting; it is
/// not worth ending the session over, and the change still holds for as long as this one runs.
async fn remember(state: &mut ChatState, config: &Config) {
  if let Err(error) = config.save().await {
    state.primary.entries.push(Entry::new(
      EntryKind::Error,
      format!("the setting holds for this session; it could not be written down: {error:#}"),
    ));
  }
}

enum CommandResult {
  Improve(String),
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
    "/improve" => Ok(CommandResult::Improve(String::new())),
    input if input.starts_with("/improve ") => Ok(CommandResult::Improve(input[9..].trim().into())),
    "/exit" | "/quit" => Ok(CommandResult::Quit),
    "/model" | "/provider" => Ok(CommandResult::Configure),
    "/config" | "/settings" => Ok(CommandResult::Settings),
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
          if header.path.as_os_str().is_empty() {
            "bundled".into()
          } else {
            header.path.display().to_string()
          }
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
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        ainz::keybind::Action::ALL
          .into_iter()
          .map(|action| {
            format!(
              "{}: {}",
              action.label(),
              ainz::keybind::label(&config.ui.keybindings, action)
            )
          })
          .collect::<Vec<_>>()
          .join("\n"),
      ));

      let listing = commands
        .iter()
        .map(|command| format!("{:<28} {}", command.usage, command.description))
        .collect::<Vec<_>>()
        .join("\n");
      state.primary.entries.push(Entry::new(
        EntryKind::System,
        format!(
          "{listing}\n\n\
           up/down    earlier prompts\n\
           @name      complete a path        esc esc      rewind a prompt\n\
           /vim /inline prompt and drawing\n\
           y n a      allow, refuse, or keep the answer at a permission prompt\n\
           ctrl+a/e   line start/end         ctrl+w       delete word\n\
           ctrl+k     cut after cursor       alt+←/→      move a word\n\
           wheel      scroll the transcript\n\
           ctrl+end   latest output          shift+↑/↓    scroll a line\n\
           ctrl+home  oldest output\n\
           esc        cancel a run\n\
           ctrl+1…9   select agent\n\
           /settings  customize shortcuts"
        ),
      ));
      Ok(CommandResult::Handled)
    }
    "/new" | "/clear" => {
      *session = Session::new(session.workspace.clone());
      state.primary.entries.clear();
      state.primary.topic.clear();
      state.outcome = None;
      state.follow();
      state.primary.live.clear();
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
    UiEvent::Approval(approval) => match state.approval {
      Some(_) => state.waiting.push_back(approval),
      None => state.approval = Some(approval),
    },
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
      view.edit(index, |entry| {
        entry.text.push_str(&text);
        entry
          .stream
          .get_or_insert_with(termweave::Stream::default)
          .push(&text);
      });
    }
    Event::ToolStart { call } => {
      state.view_mut(session_id).finish_message();
      state
        .view_mut(session_id)
        .tools
        .insert(call.id.clone(), call.name.clone());
      let subject = tool_subject(&call.name, &call.arguments);
      let subject = fit_subject(&state.relative(&subject));
      let view = state.view_mut(session_id);
      view.entries.push(Entry::call(call.name.clone(), subject));
      view
        .live
        .insert(call.id.clone(), (view.entries.len() - 1, call.name.clone()));
    }
    // the last line a running tool wrote, under the call that is writing it
    Event::ToolDelta { id, text } => {
      let view = state.view_mut(session_id);
      let Some((index, _)) = view.live.get(&id).cloned() else {
        return;
      };
      view.edit(index, |entry| {
        let detail = entry.detail.get_or_insert_with(String::new);
        detail.push_str(&text);
        // what a call writes is unbounded; what the buffer keeps for it is not
        if detail.len() > MAX_TOOL_OUTPUT {
          let start = detail.len() - MAX_TOOL_OUTPUT;
          let start = (start..detail.len())
            .find(|at| detail.is_char_boundary(*at))
            .unwrap_or(detail.len());
          detail.replace_range(..start, "[earlier output dropped]\n");
        }
        let last = detail
          .lines()
          .rfind(|line| !line.trim().is_empty())
          .unwrap_or_default()
          .to_string();
        if let Some(tool) = entry.tool.as_mut() {
          tool.live = clip(&last, 160);
        }
      });
    }
    Event::ToolEnd { id, output, error } => {
      let view = state.view_mut(session_id);
      view.tools.remove(&id);
      // the result belongs to the call that asked for it, not to a second line further down
      let Some((index, _)) = view.live.remove(&id) else {
        return;
      };
      view.edit(index, |entry| {
        if let Some(tool) = entry.tool.as_mut() {
          tool.state = match error {
            true => ToolState::Failed,
            false => ToolState::Done,
          };
          tool.live.clear();
          tool.note = tool_note(&output, error, tool.started.elapsed());
        }
        if entry.detail.is_none() && !output.trim().is_empty() {
          entry.detail = Some(output);
        }
        entry.kind = match error {
          true => EntryKind::Error,
          false => EntryKind::Tool,
        };
      });
    }
    Event::SubagentStart {
      session_id,
      name,
      task,
      ..
    } => {
      let view = state
        .agents
        .entry(session_id)
        .or_insert_with(|| AgentView::new(AgentState::Running));
      view.state = AgentState::Running;
      view.name = name;
      view.topic = title::excerpt(&task);
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
      view.finish_message();
    }
    Event::Compaction {
      archived_messages, ..
    } => {
      state.view_mut(session_id).entries.push(Entry::new(
        EntryKind::System,
        format!("compacted {archived_messages} messages"),
      ));
    }
    Event::Steering { message } => {
      if session_id.is_none()
        && let Some(index) = state.steers.iter().position(|queued| *queued == message)
      {
        state.steers.remove(index);
      }
      let view = state.view_mut(session_id);
      view.topic = title::excerpt(&message);
      view.entries.push(Entry::new(
        EntryKind::System,
        format!("steering applied: {message}"),
      ));
    }
    Event::Cancelled => {
      if session_id.is_none() {
        state.cancelled = true;
      } else {
        state
          .view_mut(session_id)
          .entries
          .push(Entry::new(EntryKind::Cancelled, "Run cancelled".into()));
      }
    }
    Event::Error { message } => state
      .view_mut(session_id)
      .entries
      .push(Entry::new(EntryKind::Error, message)),
    Event::Notice { message } => state
      .view_mut(session_id)
      .entries
      .push(Entry::new(EntryKind::System, message)),
    Event::TurnEnd { usage } => {
      let view = state.view_mut(session_id);
      view.usage.input_tokens += usage.input_tokens;
      view.usage.output_tokens += usage.output_tokens;
      view.finish_message();
    }
    Event::Todos { items } => state.view_mut(session_id).todos = items,
    Event::SessionStart { .. } | Event::ThinkingDelta { .. } => {}
  }
}

// the conversation as it stands, which after a rewind is not every node ever written
fn active_path(session: &Session) -> Vec<&ainz::session::SessionNode> {
  // by id rather than by scanning: a long session holds thousands of nodes, and this walks the
  // whole of the active branch
  let nodes: std::collections::HashMap<_, _> =
    session.nodes.iter().map(|node| (node.id, node)).collect();
  let mut path = Vec::new();
  let mut current = session.cursor;
  while let Some(id) = current {
    let Some(node) = nodes.get(&id) else {
      break;
    };
    path.push(*node);
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

// the plan as the last call that set one left it, so a resumed session shows where it was
fn session_plan(session: &Session) -> Vec<ainz::Todo> {
  active_path(session)
    .iter()
    .rev()
    .flat_map(|node| node.message.tool_calls.iter().rev())
    .find_map(ainz::todo::planned)
    .unwrap_or_default()
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
  let status_rows = if config.ui.status_line.as_ref().is_some_and(Vec::is_empty) {
    0
  } else {
    1
  };
  // the prompt grows with what is written in it, up to a few lines
  let prompt_rows = input::wrap(state.input.as_str(), prompt_width(frame.area().width))
    .len()
    .clamp(1, 6) as u16;
  if state.inline {
    let [body, activity, status, input] = Layout::vertical([
      Constraint::Min(1),
      Constraint::Length(1),
      Constraint::Length(status_rows),
      Constraint::Length(prompt_rows),
    ])
    .areas(frame.area());
    render_transcript(frame, body, state);
    activity::render(frame, activity, state);
    render_status(frame, status, state, config);
    render_input(frame, input, state);
    render_command_palette(frame, body, status, state, commands);
    if let Some(approval) = &state.approval {
      render_approval(frame, approval);
    }
    return;
  }
  let [header, body, activity, status, input] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Min(8),
    Constraint::Length(1),
    Constraint::Length(status_rows),
    Constraint::Length(prompt_rows),
  ])
  .areas(frame.area());
  title::render(frame, header, state, session_id, workspace);
  let transcript = if state.roster && body.width >= 72 {
    let width = match state.roster_source {
      RosterSource::Local => 24,
      RosterSource::Mesh => 34,
    };
    let [transcript, roster] = Layout::horizontal([Constraint::Min(30), Constraint::Length(width)])
      .spacing(1)
      .areas(body);
    match state.roster_source {
      RosterSource::Local => render_roster(frame, roster, state),
      RosterSource::Mesh => render_mesh_roster(frame, roster, state),
    }
    transcript
  } else {
    body
  };
  render_transcript(frame, transcript, state);
  activity::render(frame, activity, state);
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
  if let Some(mut art) = state.art_area.get() {
    art.height = art.height.min(area.y.saturating_sub(art.y));
    state.art_area.set(Some(art));
  }
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
          (*path).to_string(),
          "file".to_string(),
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

fn render_transcript(frame: &mut Frame, area: Rect, state: &ChatState) {
  if state.header_preview {
    let lines = state.splash(area.width.saturating_sub(1) as usize, area.height as usize);
    state.art_area.set(Some(Rect::new(
      area.x + 1,
      area.y,
      area.width.saturating_sub(1),
      (lines.len().saturating_sub(3) as u16).min(area.height),
    )));
    frame.render_widget(
      Paragraph::new(lines).block(
        Block::default()
          .borders(Borders::LEFT)
          .border_style(Style::default().fg(BLUE)),
      ),
      area,
    );
    return;
  }
  let view = state.active_view();
  let entries = if state.inline && state.active.is_none() {
    &view.entries[state.flushed.min(view.entries.len())..]
  } else {
    &view.entries[..]
  };
  let width = area.width.saturating_sub(1) as usize;
  let block = Block::default()
    .borders(Borders::LEFT)
    .border_style(Style::default().fg(BLUE));
  let inner = block.inner(area);
  frame.render_widget(block, area);
  if entries.is_empty() {
    if state.inline {
      return;
    }
    let lines = state.splash(width, area.height as usize);
    state.art_area.set(Some(Rect::new(
      area.x + 1,
      area.y,
      area.width.saturating_sub(1),
      (lines.len().saturating_sub(3) as u16).min(area.height),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
    return;
  }
  // heights come from what each entry remembers, so the whole buffer can be measured every
  // frame without laying any of it out again
  let heights: Vec<usize> = entries
    .iter()
    .map(|entry| entry.height(state.expanded, width))
    .collect();
  let total: usize = heights.iter().sum();
  let height = inner.height as usize;
  let bottom = total.saturating_sub(height);
  state.page.set(inner.height);
  let offset = view.entries.len() - entries.len();
  let (skip, scroll) = settle(
    state.scroll.get(),
    state.scroll_delta.replace(0),
    &heights,
    offset,
    bottom,
  );
  state.scroll.set(scroll);
  // only the entries the buffer can actually show are laid out; the rest let go of their rows
  // and keep only the height, so what a long session holds does not grow with it
  let mut shown: Vec<Line<'static>> = Vec::with_capacity(height);
  let mut passed = 0;
  for (entry, rows) in entries.iter().zip(&heights) {
    let visible = passed < skip + height && passed + rows > skip;
    if !visible || shown.len() == height {
      entry.release();
      passed += rows;
      continue;
    }
    let lines = entry.lines(state.expanded, width);
    for line in lines.iter().skip(skip.saturating_sub(passed)) {
      if shown.len() == height {
        break;
      }
      shown.push(line.clone());
    }
    passed += rows;
  }
  frame.render_widget(Paragraph::new(shown), inner);
}

/// Where a reader is in the buffer: following the newest output, or holding the row at the top
/// of the screen. The row is named by the entry it belongs to rather than by a count from
/// either end, so output that grows, shrinks, re-wraps on a resize, or is expanded with ctrl+o
/// leaves the reader looking at the same thing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scroll {
  #[default]
  Follow,
  At {
    entry: usize,
    row: usize,
  },
}

/// The rows to skip this frame, and where that leaves the reader. `offset` is how many entries
/// have already left for the terminal's own scrollback and are not in `heights`.
fn settle(
  scroll: Scroll,
  delta: isize,
  heights: &[usize],
  offset: usize,
  bottom: usize,
) -> (usize, Scroll) {
  let current = match scroll {
    Scroll::Follow => bottom,
    Scroll::At { entry, row } => match entry.checked_sub(offset) {
      Some(local) if local < heights.len() => {
        heights[..local].iter().sum::<usize>() + row.min(heights[local].saturating_sub(1))
      }
      _ => bottom,
    },
  };
  let skip = current.saturating_add_signed(delta).min(bottom);
  // everything fits, or the reader came back down to the end: newest output is followed again
  if bottom == 0 || (skip == bottom && (delta > 0 || scroll == Scroll::Follow)) {
    return (bottom, Scroll::Follow);
  }
  let mut passed = 0;
  for (index, rows) in heights.iter().enumerate() {
    if passed + rows > skip {
      return (
        skip,
        Scroll::At {
          entry: offset + index,
          row: skip - passed,
        },
      );
    }
    passed += rows;
  }
  (bottom, Scroll::Follow)
}

fn splash(
  style: usize,
  custom: Option<&HeaderArt>,
  width: usize,
  height: usize,
) -> Vec<Line<'static>> {
  let mut lines = custom
    .and_then(|header| header.fitted_lines(width, height.saturating_sub(3)))
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
      format!(
        "/help commands · ctrl+1…9 select · ctrl+l local · {} mesh",
        ainz::keybind::Action::MeshRoster.default_key()
      ),
      Style::default().fg(MUTED),
    )
    .alignment(Alignment::Center),
  ]);
}

fn selected_header(preference: &str, catalog: &HeaderCatalog) -> (usize, Option<HeaderArt>) {
  let style = masthead::select_index(masthead::VARIANTS);
  match preference {
    "builtin" => {
      let bundled: Vec<_> = catalog
        .headers
        .iter()
        .filter(|header| header.path.as_os_str().is_empty())
        .collect();
      let choice = masthead::select_index(masthead::VARIANTS + bundled.len());
      (
        style,
        choice
          .checked_sub(masthead::VARIANTS)
          .map(|index| bundled[index].clone()),
      )
    }
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
  // the plan of whichever transcript is on screen sits under the agents, given whatever room
  // they leave
  let todos = &state.active_view().todos;
  let (agents, plan) = match todos.is_empty() {
    true => (area, Rect::default()),
    false => {
      let rows = (items.len() as u16 + 2).min(area.height / 2);
      let [agents, plan] =
        Layout::vertical([Constraint::Length(rows), Constraint::Min(0)]).areas(area);
      (agents, plan)
    }
  };
  frame.render_widget(
    List::new(items).block(
      Block::default()
        .title(" agents ")
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(BLUE)),
    ),
    agents,
  );
  if plan.height > 1 {
    render_plan(frame, plan, todos);
  }
}

fn render_mesh_roster(frame: &mut Frame, area: Rect, state: &ChatState) {
  let block = Block::default()
    .title(" synapse mesh ")
    .borders(Borders::LEFT)
    .border_style(Style::default().fg(BLUE));
  let inner = block.inner(area);
  frame.render_widget(block, area);
  let Some(roster) = &state.mesh_roster else {
    let message = state
      .mesh_error
      .as_deref()
      .unwrap_or("Use the mesh roster shortcut to load Synapse. Enable it in /settings first.");
    frame.render_widget(
      Paragraph::new(message)
        .style(Style::default().fg(MUTED))
        .wrap(Wrap { trim: false }),
      inner,
    );
    return;
  };
  if !roster.enabled {
    frame.render_widget(
      Paragraph::new("The mesh is off. Turn on Agent mesh in /settings, then refresh this roster.")
        .style(Style::default().fg(MUTED))
        .wrap(Wrap { trim: false }),
      inner,
    );
    return;
  }

  let mut items = Vec::new();
  if !roster.workers.is_empty() {
    items.push(ListItem::new(Line::styled(
      format!(" workers {}", roster.workers.len()),
      Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    )));
  }
  for worker in &roster.workers {
    let agent = roster.agents.iter().find(|agent| agent.name == worker.name);
    let (marker, color, status) = match agent {
      Some(agent) if agent.online && agent.status.eq_ignore_ascii_case("working") => {
        ("+", ACTIVE, "working")
      }
      Some(agent) if agent.online && agent.status.eq_ignore_ascii_case("blocked") => {
        ("!", YELLOW, "blocked")
      }
      Some(agent) if agent.online => ("+", CYAN, agent.status.as_str()),
      _ if worker.status == "running" => ("?", YELLOW, "unresponsive"),
      _ => ("·", MUTED, "stopped"),
    };
    let detail = if status.is_empty() { "online" } else { status };
    items.push(ListItem::new(Line::from(vec![
      Span::styled(format!(" {marker} "), Style::default().fg(color)),
      Span::styled(clip(&worker.name, 18), Style::default().fg(color)),
      Span::styled(format!(" {detail}"), Style::default().fg(MUTED)),
    ])));
  }

  let worker_names = roster
    .workers
    .iter()
    .map(|worker| worker.name.as_str())
    .collect::<std::collections::HashSet<_>>();
  let available = roster
    .agents
    .iter()
    .filter(|agent| agent.online && !worker_names.contains(agent.name.as_str()))
    .collect::<Vec<_>>();
  if !available.is_empty() {
    items.push(ListItem::new(Line::styled(
      format!(" other agents {}", available.len()),
      Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    )));
  }
  for agent in available {
    let status = if agent.human {
      "you"
    } else if agent.status.is_empty() {
      "online"
    } else {
      agent.status.as_str()
    };
    items.push(ListItem::new(Line::from(vec![
      Span::styled(" + ", Style::default().fg(CYAN)),
      Span::styled(clip(&agent.name, 18), Style::default().fg(CYAN)),
      Span::styled(format!(" {status}"), Style::default().fg(MUTED)),
    ])));
  }
  let hidden = roster
    .agents
    .iter()
    .filter(|agent| !agent.online && !worker_names.contains(agent.name.as_str()))
    .count();
  if hidden > 0 {
    items.push(ListItem::new(Line::styled(
      format!(" {hidden} offline hidden"),
      Style::default().fg(MUTED),
    )));
  }
  if items.is_empty() {
    items.push(ListItem::new(Line::styled(
      " no agents have joined",
      Style::default().fg(MUTED),
    )));
  }
  frame.render_widget(List::new(items), inner);
}

fn render_plan(frame: &mut Frame, area: Rect, todos: &[ainz::Todo]) {
  let width = usize::from(area.width.saturating_sub(5));
  let items: Vec<_> = todos
    .iter()
    .map(|todo| {
      let (marker, style) = match todo.status {
        ainz::TodoStatus::Done => ("[x] ", Style::default().fg(MUTED)),
        ainz::TodoStatus::InProgress => (
          "[>] ",
          Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
        ),
        ainz::TodoStatus::Pending => ("[ ] ", Style::default().fg(INK)),
      };
      ListItem::new(Line::from(vec![
        Span::styled(marker, style),
        Span::styled(clip(&todo.text, width), style),
      ]))
    })
    .collect();
  let done = todos
    .iter()
    .filter(|todo| todo.status == ainz::TodoStatus::Done)
    .count();
  frame.render_widget(
    List::new(items).block(
      Block::default()
        .title(format!(" plan {done}/{} ", todos.len()))
        .borders(Borders::LEFT)
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
  let model = &config.model;
  // what memory this session has, since it changes what the model can be expected to know
  let memory = match (config.memory.backend, config.mesh_active()) {
    (ainz::MemoryBackend::Off, _) => String::new(),
    (backend, true) => format!(" │ {} +mesh", backend.label()),
    (backend, false) => format!(" │ {}", backend.label()),
  };
  // only a provider that reports what a turn cost has one to show
  let cost = view
    .usage
    .cost_usd
    .map(|cost| format!(" │ ${cost:.2}"))
    .unwrap_or_default();
  // the roster shows the plan itself; this is for when it is hidden or too narrow to
  let plan = match view.todos.len() {
    0 => String::new(),
    total => {
      let done = view
        .todos
        .iter()
        .filter(|todo| todo.status == ainz::TodoStatus::Done)
        .count();
      format!(" │ todo {done}/{total}")
    }
  };
  let attention = match state.attention {
    (0, 0, 0) => String::new(),
    (approval, failed, done) => format!("attention {approval} ask / {failed} failed / {done} done"),
  };
  let text = if let Some(items) = &config.ui.status_line {
    use ainz::config::StatusItem;
    let fields: Vec<String> = items
      .iter()
      .filter_map(|item| {
        let text = match item {
          StatusItem::Provider => provider.into(),
          StatusItem::Model => model.clone(),
          StatusItem::Mode => permissions.into(),
          StatusItem::Memory => config.memory.backend.label().into(),
          StatusItem::Tokens => format!("Σ{total} tok"),
          StatusItem::InputTokens => format!("in {input}"),
          StatusItem::OutputTokens => format!("out {output}"),
          StatusItem::Context => {
            format!("limit {} tok", compact_count(config.context_tokens as u64))
          }
          StatusItem::Todos => plan.trim_start_matches(" │ ").into(),
          StatusItem::Agents => format!("agents {running}/{total_agents}"),
          StatusItem::Workspace => state
            .workspace
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
          StatusItem::Session => state.session_name.clone().unwrap_or_default(),
          StatusItem::Activity => if state.approval.is_some() {
            "needs approval"
          } else if state.busy() {
            "running"
          } else {
            "ready"
          }
          .into(),
          StatusItem::Attention => attention.clone(),
          StatusItem::Branch => state.branch.clone().unwrap_or_default(),
        };
        (!text.is_empty()).then_some(text)
      })
      .collect();
    format!(" {} ", fields.join(" │ "))
  } else if !attention.is_empty() {
    format!(" {attention} │ {model} │ Σ{total} tok ")
  } else if area.width >= 108 {
    format!(
      " {provider}/{model} │ {permissions}{memory} │ tokens in {input} out {output} total \
       {total}{cost} │ agents {running}/{total_agents}{plan} │ {} local · {} mesh ",
      ainz::keybind::label(&config.ui.keybindings, ainz::keybind::Action::Roster),
      ainz::keybind::label(&config.ui.keybindings, ainz::keybind::Action::MeshRoster)
    )
  } else if area.width >= 72 {
    format!(
      " {provider}/{model} │ {permissions} │ tok {input}↓ {output}↑ Σ{total}{cost} │ \
       ag {running}/{total_agents}{plan} "
    )
  } else {
    format!(" {} │ Σ{total} tok ", config.model)
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
/// A pasted path to an image, however the terminal spelled it: quoted, escaped, or a file URL.
fn image_path(text: &str) -> Option<PathBuf> {
  const KINDS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
  let text = text.trim().trim_matches(['"', '\'']);
  let text = text.strip_prefix("file://").unwrap_or(text);
  let cleaned = text.replace("\\ ", " ");
  let path = PathBuf::from(&cleaned);
  let extension = path.extension()?.to_str()?.to_ascii_lowercase();
  (KINDS.contains(&extension.as_str()) && path.is_file()).then_some(path)
}

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
/// Borrowed rather than owned: this is rebuilt on every keystroke and again on every frame,
/// and none of it outlives the frame it is drawn in.
enum Suggestion<'a> {
  Command(&'a SlashCommand),
  Path(&'a str),
}

/// Whether a path contains what was typed, ignoring case, without folding a copy of it. The
/// workspace listing runs to thousands of paths and this is asked of all of them on every
/// keystroke; allocating there is what makes completion feel slow.
fn contains_fold(haystack: &str, needle: &str) -> bool {
  if needle.is_empty() {
    return true;
  }
  let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
  let Some(last) = haystack.len().checked_sub(needle.len()) else {
    return false;
  };
  (0..=last).any(|at| {
    haystack[at..at + needle.len()]
      .iter()
      .zip(needle)
      .all(|(left, right)| left.eq_ignore_ascii_case(right))
  })
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

fn suggestions<'a>(state: &'a ChatState, commands: &'a [SlashCommand]) -> Vec<Suggestion<'a>> {
  let Some((_, fragment)) = file_fragment(state.input.as_str(), state.input.cursor()) else {
    return command_matches(commands, state.input.as_str())
      .into_iter()
      .map(Suggestion::Command)
      .collect();
  };
  let mut matched: Vec<_> = state
    .files
    .iter()
    .filter(|path| contains_fold(path, &fragment))
    .collect();
  // a name that starts the way you typed comes before one that merely contains it
  matched.sort_by_key(|path| {
    let name = path.rsplit('/').next().unwrap_or(path.as_str());
    let leading = !name
      .get(..fragment.len())
      .is_some_and(|start| start.eq_ignore_ascii_case(&fragment));
    (leading, path.len())
  });
  matched
    .into_iter()
    .take(20)
    .map(|path| Suggestion::Path(path.as_str()))
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

/// What the call acts on, as it would be written on a command line. The key order is what
/// these tools name their subject, whichever harness sent the call.
fn tool_subject(name: &str, arguments: &serde_json::Value) -> String {
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
  let _ = name;
  let subject = SUBJECT.iter().find_map(|key| {
    arguments
      .get(key)
      .and_then(serde_json::Value::as_str)
      .map(str::trim)
      .filter(|value| !value.is_empty())
  });
  subject.map(str::to_string).unwrap_or_default()
}

/// The subject as it fits on one line: a path is identified by its end, so an overlong one
/// keeps its tail rather than its root.
fn fit_subject(subject: &str) -> String {
  match subject.contains('/') && !subject.contains(char::is_whitespace) {
    true => clip_start(subject, 72),
    false => clip(subject, 72),
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

fn output_is_json(value: &str) -> bool {
  let value = value.trim();
  if value.is_empty() {
    return false;
  }
  value
    .chars()
    .next()
    .is_some_and(|ch| matches!(ch, '{' | '['))
    && serde_json::from_str::<serde_json::Value>(value).is_ok()
}

fn output_summary(value: &str) -> &'static str {
  if !output_is_json(value) {
    return "";
  }
  match serde_json::from_str::<serde_json::Value>(value.trim()) {
    Ok(serde_json::Value::Array(values)) => {
      if values.is_empty() {
        "JSON array (empty)"
      } else {
        "JSON array"
      }
    }
    Ok(serde_json::Value::Object(values)) => {
      if values.is_empty() {
        "JSON object (empty)"
      } else {
        "JSON object"
      }
    }
    Ok(_) => "JSON value",
    Err(_) => "JSON-like output",
  }
}

/// What the call cost, once it is over: how long it took, and how much it wrote or why it did
/// not. That is the part worth reading when the output itself is not.
fn tool_note(output: &str, error: bool, took: Duration) -> String {
  let seconds = took.as_secs_f64();
  let spent = match seconds {
    seconds if seconds < 1.0 => format!("{}ms", took.as_millis()),
    seconds if seconds < 60.0 => format!("{seconds:.1}s"),
    seconds => elapsed(seconds as u64),
  };
  if error {
    if output_is_json(output) {
      return format!("{spent} · {}", output_summary(output));
    }
    let first = output
      .lines()
      .find(|line| !line.trim().is_empty())
      .unwrap_or("failed");
    return format!("{spent} · {}", clip(first, 80));
  }
  if output_is_json(output) {
    return format!("{spent} · {}", output_summary(output));
  }
  match output
    .lines()
    .filter(|line| !line.trim().is_empty())
    .count()
  {
    0 => spent,
    1 => format!("{spent} · {}", clip(output.trim(), 80)),
    lines => format!("{spent} · {lines} lines"),
  }
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
  if let Some(path) = &state.pending_image {
    let name = path
      .file_name()
      .map(|name| name.to_string_lossy().to_string())
      .unwrap_or_default();
    frame.render_widget(
      Paragraph::new(Line::styled(
        format!(" 🖼 {name}"),
        Style::default().fg(MAGENTA),
      ))
      .alignment(Alignment::Right),
      area,
    );
  }
  let prefix = match (state.busy(), state.input.mode()) {
    (true, _) => "↳ ",
    (false, input::Mode::Normal) => "▪ ",
    (false, input::Mode::Insert) => "> ",
  };
  let width = prompt_width(area.width);
  let text = state.input.as_str();
  let rows = input::wrap(text, width);
  let (row, column) = state.input.caret(width);
  // the prompt holds a few rows; past that it scrolls to keep the caret in view
  let first = row.saturating_sub(usize::from(area.height.max(1)) - 1);
  state.prompt_first.set(first);
  let lines: Vec<Line> = rows
    .iter()
    .enumerate()
    .skip(first)
    .take(usize::from(area.height))
    .map(|(index, range)| {
      Line::from(vec![
        Span::styled(
          if index == 0 { prefix } else { "  " },
          Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(&text[range.clone()], Style::default().fg(INK)),
      ])
    })
    .collect();
  frame.render_widget(Paragraph::new(lines), area);
  let x = area
    .x
    .saturating_add(2)
    .saturating_add(column.min(u16::MAX as usize) as u16)
    .min(area.right().saturating_sub(1));
  let y = area
    .y
    .saturating_add((row - first).min(u16::MAX as usize) as u16)
    .min(area.bottom().saturating_sub(1));
  frame.set_cursor_position((x, y));
}

// the prompt's text starts after its two-column marker and leaves a column for the caret
fn prompt_width(width: u16) -> usize {
  usize::from(width.saturating_sub(3).max(1))
}

// the arguments are the decision, so they are shown, clipped to what fits
/// What the call would actually do, in the shape the decision is about: a change to a file
/// reads as a diff, a command as the command. Raw JSON is the last resort, not the first.
fn approval_preview(call: &ToolCall) -> Vec<Line<'static>> {
  let field = |key: &str| {
    call
      .arguments
      .get(key)
      .and_then(serde_json::Value::as_str)
      .unwrap_or_default()
      .to_string()
  };
  let marked = |text: String, mark: &'static str, color: Color| {
    text
      .lines()
      .take(6)
      .map(|line| {
        Line::from(vec![
          Span::styled(mark, Style::default().fg(color)),
          Span::styled(clip(line, 80), Style::default().fg(color)),
        ])
      })
      .collect::<Vec<_>>()
  };
  match call.name.as_str() {
    "edit" => {
      let mut lines = vec![Line::styled(field("path"), Style::default().fg(INK))];
      lines.extend(marked(field("old"), "- ", RED));
      lines.extend(marked(field("new"), "+ ", ACTIVE));
      lines
    }
    "write" => {
      let mut lines = vec![Line::styled(field("path"), Style::default().fg(INK))];
      lines.extend(marked(field("content"), "+ ", ACTIVE));
      lines
    }
    "shell" => vec![Line::styled(
      clip(&field("command"), 200),
      Style::default().fg(INK),
    )],
    _ => vec![Line::styled(
      clip(&call.arguments.to_string(), 240),
      Style::default().fg(INK),
    )],
  }
}

fn render_approval(frame: &mut Frame, approval: &Approval) {
  let preview = approval_preview(&approval.call);
  let height = (preview.len() as u16 + 8).min(frame.area().height);
  let area = centered(frame.area(), frame.area().width.min(88), height);
  frame.render_widget(Clear, area);
  let mut lines = vec![
    Line::styled(
      "permission required",
      Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
    ),
    Line::raw(""),
    Line::from(vec![
      Span::styled(
        approval.call.name.clone(),
        Style::default().fg(INK).add_modifier(Modifier::BOLD),
      ),
      Span::styled(format!("  {:?}", approval.risk), Style::default().fg(MUTED)),
    ]),
  ];
  lines.extend(preview);
  lines.extend([
    Line::raw(""),
    Line::styled(
      format!("a  always allow {}", approval_rule(approval)),
      Style::default().fg(MUTED),
    ),
    Line::styled("y allow   a always   n deny", Style::default().fg(ACCENT)),
  ]);
  frame.render_widget(
    Paragraph::new(lines).wrap(Wrap { trim: false }).block(
      Block::default()
        .title(" approval ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(YELLOW))
        .padding(Padding::horizontal(2)),
    ),
    area,
  );
}

/// The time a line was written, on the clock the person reading it is using. A buffer stamped
/// in UTC is wrong for everyone who is not in it, and the stamp is on every row.
fn clock() -> String {
  let epoch = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs();
  let Ok(time) = i64::try_from(epoch) else {
    return "--:--".into();
  };
  let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
  // SAFETY: localtime_r fills the tm it is given and reads the time_t; both are ours and live
  // for the call. The _r form keeps no global state, so it is safe from any thread. The struct
  // is only read back on the non-null return, which is what says it was filled.
  let filled = unsafe { libc::localtime_r(&time, local.as_mut_ptr()) };
  if filled.is_null() {
    let seconds = epoch % 86_400;
    return format!("{:02}:{:02}", seconds / 3600, (seconds % 3600) / 60);
  }
  // SAFETY: localtime_r returned non-null, so it filled the struct
  let local = unsafe { local.assume_init() };
  format!("{:02}:{:02}", local.tm_hour, local.tm_min)
}

#[cfg(test)]
#[path = "../../tests/tui/chat.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/tui/wrap.rs"]
mod wrapping;
