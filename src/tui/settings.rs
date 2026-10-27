use ainz::{Config, HeaderCatalog, MemoryBackend, PermissionMode, synapse};
use ainz::{
  config::{NotificationDelivery, Notifications},
  keybind::{self, Action},
};
use anyhow::Result;
use crossterm::event::{
  self, Event as InputEvent, KeyCode, KeyEventKind, MouseEventKind as InputMouse,
};
use ratatui::{
  Frame,
  layout::{Constraint, Layout},
  style::{Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
};

use super::{ACCENT, ACTIVE, INK, MUTED, Term, YELLOW, enter_terminal, leave_terminal};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
  Provider,
  Wire,
  Reasoning,
  Fallback,
  Permissions,
  Memory,
  RecallOnStart,
  RememberOnCompact,
  Teach,
  Improve,
  Synapse,
  Mesh,
  Roster,
  Header,
  Vim,
  Inline,
  Bell,
  Background,
  Notifications,
  Delivery,
  Status,
  Binding(Action),
}

const ROWS: [Row; 21] = [
  Row::Provider,
  Row::Wire,
  Row::Reasoning,
  Row::Fallback,
  Row::Permissions,
  Row::Memory,
  Row::RecallOnStart,
  Row::RememberOnCompact,
  Row::Teach,
  Row::Improve,
  Row::Synapse,
  Row::Mesh,
  Row::Roster,
  Row::Header,
  Row::Vim,
  Row::Inline,
  Row::Bell,
  Row::Background,
  Row::Notifications,
  Row::Delivery,
  Row::Status,
];

impl Row {
  fn label(self) -> &'static str {
    match self {
      Self::Background => "Background sessions",
      Self::Notifications => "Notifications",
      Self::Delivery => "Notification delivery",
      Self::Status => "Status bar fields",
      Self::Binding(action) => action.label(),
      Self::Provider => "Provider and model",
      Self::Wire => "Provider API",
      Self::Reasoning => "Reasoning effort",
      Self::Fallback => "Fallback providers",
      Self::Permissions => "Permissions",
      Self::Memory => "Memory",
      Self::RecallOnStart => "Recall at session start",
      Self::RememberOnCompact => "Remember before compaction",
      Self::Teach => "Learn reusable procedures",
      Self::Improve => "Automatic improvement",
      Self::Synapse => "Synapse connection",
      Self::Mesh => "Agent mesh",
      Self::Roster => "Agent roster",
      Self::Header => "Header art",
      Self::Bell => "Bell when done",
      Self::Vim => "Vim keys",
      Self::Inline => "Draw inline",
    }
  }

  fn value(self, config: &Config, installed: bool) -> String {
    match self {
      Self::Background => on_off(config.ui.background),
      Self::Notifications => config.ui.notifications.label().into(),
      Self::Delivery => config.ui.notification_delivery.label().into(),
      Self::Status => match &config.ui.status_line {
        None => "automatic".into(),
        Some(items) if items.is_empty() => "hidden".into(),
        Some(items) => format!("{} fields", items.len()),
      },
      Self::Binding(action) => keybind::label(&config.ui.keybindings, action).into(),
      Self::Wire => config
        .active_provider()
        .map(|provider| format!("{:?}", provider.wire_api).to_lowercase())
        .unwrap_or_else(|_| "unconfigured".into()),
      Self::Reasoning => config
        .active_provider()
        .ok()
        .and_then(|provider| provider.reasoning_effort)
        .unwrap_or_else(|| "default".into()),
      Self::Fallback => {
        if config.fallback_providers.is_empty() {
          "off".into()
        } else {
          config.fallback_providers.join(", ")
        }
      }
      Self::Provider => format!(
        "{} · {}",
        config.provider.as_deref().unwrap_or("default"),
        if config.model.is_empty() {
          "no model"
        } else {
          &config.model
        }
      ),
      Self::Permissions => match config.permissions {
        PermissionMode::Ask => "ask".into(),
        PermissionMode::Auto => "auto".into(),
        PermissionMode::ReadOnly => "read-only".into(),
      },
      Self::Memory => match config.memory.backend {
        MemoryBackend::Synapse if !installed => "synapse · not installed".into(),
        backend => backend.label().into(),
      },
      Self::RecallOnStart => on_off(config.memory.recall_on_start),
      Self::RememberOnCompact => on_off(config.memory.remember_on_compact),
      Self::Teach => on_off(config.memory.teach),
      Self::Improve => on_off(config.improve.enabled),
      Self::Synapse => match (config.synapse.enabled, installed) {
        (true, false) => "on · not installed".into(),
        (enabled, _) => on_off(enabled),
      },
      Self::Mesh => match (config.synapse.mesh, config.synapse_active()) {
        (true, false) => "on · needs Synapse".into(),
        (enabled, _) => on_off(enabled),
      },
      Self::Roster => {
        if config.ui.roster_visible {
          "visible".into()
        } else {
          "hidden".into()
        }
      }
      Self::Header => config.ui.header.clone(),
      Self::Bell => on_off(config.ui.bell),
      Self::Vim => on_off(config.ui.vim),
      Self::Inline => on_off(config.ui.inline),
    }
  }

  fn detail(self, config: &Config, installed: bool) -> (String, String) {
    match self {
      Self::Background => ("Applies on the next launch".into(), "Keep an agent running when you detach or your terminal closes. Ctrl+B D detaches; ainz attach reconnects. Ctrl+B S switches sessions. Each running session has its own host; there is no idle daemon. Off by default. Hosted sessions use the full-screen interface.".into()),
      Self::Notifications => ("For sessions you are not viewing".into(), "Off is quiet. Needs input alerts when approval is required. All also alerts on completion and failure. Opening a picker does not mark a completion as seen; viewing its transcript does.".into()),
      Self::Delivery => ("Where alerts appear".into(), "Terminal uses OSC 9 in an attached terminal that supports notifications. System uses the operating system notification command, including when detached. Both require Notifications to be enabled.".into()),
      Self::Status => ("The IRC bar above the prompt".into(), "Enter opens the field picker. Space toggles a field; Shift+Up/Down reorders it. Enter saves; R restores the responsive default. Turning every field off hides the bar.".into()),
      Self::Binding(action) => ("Record one shortcut or a two-key chord".into(), format!("{} Default: {}. Enter starts recording; press the combination, then Enter to save. Backspace restores the default. Delete disables optional actions. Conflicts are explained before saving. Esc cancels.", action.help(), action.default_key())),
      Self::Wire => ("The selected provider’s protocol".into(), "Chat uses Chat Completions; Responses supports encrypted reasoning; Messages uses the native Messages API. This edits a saved HTTP profile.".into()),
      Self::Reasoning => ("For Responses providers".into(), "Choose a reasoning effort supported by your model. Default leaves the decision to the provider.".into()),
      Self::Fallback => ("Explicit backup profiles in order".into(), "Enter configured provider names separated by commas. Each backup can have its own default model. Empty disables fallback. Failed attempts are buffered, so only a successful reply reaches the transcript.".into()),
      Self::Provider => (
        "Where completions come from".into(),
        "Open the provider setup to add or switch a provider and choose its model.".into(),
      ),
      Self::Permissions => (
        "What a tool may do without asking".into(),
        "ask prompts before writes, commands, and network calls. auto runs everything. \
         read-only refuses anything that changes the workspace."
          .into(),
      ),
      Self::Memory => (
        "Where durable context is kept".into(),
        format!(
          "off removes the memory tool. local keeps Markdown files under the Ainz data \
           directory, private to this machine. synapse stores them in Synapse, shared with \
           every other tool connected to it — {}",
          synapse::SITE
        ),
      ),
      Self::RecallOnStart => (
        "Opening a session already knowing".into(),
        "Recalls what was written down for this workspace and puts it in the system prompt \
         before the first message."
          .into(),
      ),
      Self::RememberOnCompact => (
        "The moment forgetting costs something".into(),
        "When the transcript is compacted, the session is asked to write down anything it \
         worked out that is not stored yet."
          .into(),
      ),
      Self::Teach => (
        "Skills the session writes".into(),
        "Adds a learn tool so a session can write down a procedure it worked out, and correct \
         one that turned out wrong. A new skill waits for approval — `ainz skills proposed` \
         lists them — so writing one costs a line in a list rather than context everywhere."
          .into(),
      ),
      Self::Improve => (
        "Learn from normal runs in the background".into(),
        format!("After {} new saved runs, test one harness change and propose skills. Experiments edit separate source copies and run checks without prompts. Uses the current model, at most {} repair steps and six steps per behavioral task, sharing ${:.2} reported cost and {} seconds. Every fixed behavioral task must pass before acceptance. Cooldown: {} seconds. Source and checks are found automatically. /improve reviews results; /improve approve accepts the reviewed source patch and skills. No automatic installation.", config.improve.every_runs, config.improve.max_steps, config.improve.budget_usd, config.improve.max_seconds, config.improve.cooldown_seconds),
      ),
      Self::Synapse => (
        if installed {
          "Installed".into()
        } else {
          "Not installed".into()
        },
        format!(
          "{} Turning this on connects both products: Ainz loads Synapse's SOUL.md guidance \
           and shared library, while Synapse can see Ainz and its subagents. Choose synapse as \
           the Memory backend to share durable context too. Cmd+R opens the machine-wide mesh \
           roster. Optional: Ainz runs the same without it. {}",
          synapse::SUMMARY,
          synapse::SITE
        ),
      ),
      Self::Mesh => (
        "Subagents that can talk".into(),
        format!(
          "Registers this session on the Synapse mesh and gives every subagent its own seat \
           under its guardian name, so they can message each other and you can watch them \
           from the Synapse console. Needs Synapse{}. {}",
          if installed {
            ""
          } else {
            ", which is not installed"
          },
          synapse::SITE
        ),
      ),
      Self::Roster => (
        "Subagent panel".into(),
        "Show the running subagents beside the transcript. Ctrl+L toggles it during a session."
          .into(),
      ),
      Self::Bell => (
        "Saying so when you are not looking".into(),
        "Rings the terminal when a run that took more than ten seconds finishes, so a session \
         left working in another pane can say it is done."
          .into(),
      ),
      Self::Vim => (
        "Modal editing in the prompt".into(),
        "Esc leaves insert mode; hjkl, w, b, 0, $, i, a, x, dd and D work as they do in vim, \
         and k and j walk earlier prompts."
          .into(),
      ),
      Self::Inline => (
        "Where the session is drawn".into(),
        "off takes the whole screen and keeps the roster beside the transcript. on draws the \
         prompt at the bottom of the terminal's own scroll, so finished output stays in the \
         scrollback your terminal already keeps. Takes effect at the next launch."
          .into(),
      ),
      Self::Header => (
        "Splash artwork".into(),
        format!(
          "random picks from the built-ins and your own art, builtin uses only the shipped \
           ones, or name one file. Custom art lives beside the workspace in .ainz/headers. \
           Current: {}",
          config.ui.header
        ),
      ),
    }
  }

  // enter and the arrow keys move a row through its values; only the provider row leaves
  fn cycle(self, config: &mut Config, forward: bool, headers: &[String]) -> bool {
    match self {
      Self::Background => config.ui.background = !config.ui.background,
      Self::Notifications => {
        config.ui.notifications = step(
          &[
            Notifications::Off,
            Notifications::NeedsInput,
            Notifications::All,
          ],
          config.ui.notifications,
          forward,
        )
      }
      Self::Delivery => {
        config.ui.notification_delivery = step(
          &[NotificationDelivery::Terminal, NotificationDelivery::System],
          config.ui.notification_delivery,
          forward,
        )
      }
      Self::Status | Self::Binding(_) | Self::Wire | Self::Reasoning | Self::Fallback => {}
      Self::Provider => return true,
      Self::Permissions => {
        let modes = [
          PermissionMode::Ask,
          PermissionMode::Auto,
          PermissionMode::ReadOnly,
        ];
        config.permissions = step(&modes, config.permissions, forward);
        config.yeet = false;
      }
      Self::Memory => {
        let backends = [
          MemoryBackend::Off,
          MemoryBackend::Local,
          MemoryBackend::Synapse,
        ];
        config.memory.backend = step(&backends, config.memory.backend, forward);
        if config.memory.backend == MemoryBackend::Synapse {
          config.synapse.enabled = true;
        }
      }
      Self::RecallOnStart => config.memory.recall_on_start = !config.memory.recall_on_start,
      Self::RememberOnCompact => {
        config.memory.remember_on_compact = !config.memory.remember_on_compact;
      }
      Self::Teach => config.memory.teach = !config.memory.teach,
      Self::Improve => config.improve.enabled = !config.improve.enabled,
      Self::Synapse => {
        config.synapse.enabled = !config.synapse.enabled;
        if !config.synapse.enabled {
          config.synapse.mesh = false;
          if config.memory.backend == MemoryBackend::Synapse {
            config.memory.backend = MemoryBackend::Local;
          }
        }
      }
      Self::Mesh => {
        config.synapse.mesh = !config.synapse.mesh;
        if config.synapse.mesh {
          config.synapse.enabled = true;
        }
      }
      Self::Roster => config.ui.roster_visible = !config.ui.roster_visible,
      Self::Bell => config.ui.bell = !config.ui.bell,
      Self::Vim => config.ui.vim = !config.ui.vim,
      Self::Inline => config.ui.inline = !config.ui.inline,
      Self::Header => {
        let mut choices = vec!["random".to_string(), "builtin".to_string()];
        choices.extend(headers.iter().cloned());
        let current = choices
          .iter()
          .position(|choice| choice == &config.ui.header)
          .unwrap_or(0);
        let next = if forward {
          (current + 1) % choices.len()
        } else {
          (current + choices.len() - 1) % choices.len()
        };
        config.ui.header = choices[next].clone();
      }
    }
    false
  }
}

impl Row {
  fn section(self) -> &'static str {
    match self {
      Self::Provider | Self::Permissions | Self::Wire | Self::Reasoning | Self::Fallback => {
        "Connection"
      }
      Self::Memory
      | Self::RecallOnStart
      | Self::RememberOnCompact
      | Self::Teach
      | Self::Improve
      | Self::Synapse
      | Self::Mesh => "Memory",
      Self::Background | Self::Notifications | Self::Delivery => "Sessions",
      Self::Binding(_) => "Keys",
      _ => "Interface",
    }
  }
}
const SECTIONS: [&str; 6] = [
  "All",
  "Connection",
  "Interface",
  "Sessions",
  "Keys",
  "Memory",
];
fn rows(section: usize, query: &str) -> Vec<Row> {
  ROWS
    .into_iter()
    .chain(Action::ALL.into_iter().map(Row::Binding))
    .filter(|row| section == 0 || row.section() == SECTIONS[section])
    .filter(|row| {
      format!("{} {}", row.section(), row.label())
        .to_lowercase()
        .contains(&query.to_lowercase())
    })
    .collect()
}
fn binding(terminal: &mut Term, config: &mut Config, action: Action) -> Result<bool> {
  let mut captured = Vec::new();
  let mut problem = String::new();
  loop {
    terminal.draw(|frame| {
      frame.render_widget(Paragraph::new(format!("Shortcut: {}\n\n{}\n\n{}\n\nPress one combination or two keys, then Enter to save.\nBackspace restores default · Delete disables · Esc cancels", action.label(), captured.join(" "), problem)).style(Style::default().fg(INK)).block(Block::default().title(" Record shortcut ").borders(Borders::ALL).padding(Padding::uniform(2))), frame.area());
    })?;
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    let value = match key.code {
      KeyCode::Esc => return Ok(false),
      KeyCode::Backspace if captured.is_empty() => Some(action.default_key().to_string()),
      KeyCode::Delete if captured.is_empty() => Some("none".into()),
      KeyCode::Backspace => {
        captured.pop();
        None
      }
      KeyCode::Enter if !captured.is_empty() => Some(captured.join(" ")),
      _ if captured.len() < 2 => {
        match keybind::format(key) {
          Ok(key) => captured.push(key),
          Err(error) => problem = error.to_string(),
        };
        None
      }
      _ => None,
    };
    if let Some(value) = value {
      match keybind::set(&mut config.ui.keybindings, action, &value) {
        Ok(()) => return Ok(true),
        Err(error) => {
          problem = error.to_string();
          captured.clear();
        }
      }
    }
  }
}

fn provider_setting(terminal: &mut Term, config: &mut Config, row: Row) -> Result<bool> {
  use ainz::WireApi;
  let (items, current) = match row {
    Row::Fallback => (vec![String::new()], config.fallback_providers.join(", ")),
    Row::Wire => (
      vec!["chat".into(), "responses".into(), "messages".into()],
      config
        .active_provider()
        .map(|profile| format!("{:?}", profile.wire_api).to_lowercase())
        .unwrap_or_else(|_| "chat".into()),
    ),
    Row::Reasoning => (
      vec![
        "default".into(),
        "none".into(),
        "minimal".into(),
        "low".into(),
        "medium".into(),
        "high".into(),
        "xhigh".into(),
      ],
      config
        .active_provider()?
        .reasoning_effort
        .unwrap_or_else(|| "default".into()),
    ),
    _ => return Ok(false),
  };
  let Some(value) = super::choose_value(
    terminal,
    row.label(),
    "Select a value or type another; Esc cancels",
    "Value",
    items,
    &current,
  )?
  else {
    return Ok(false);
  };
  if row == Row::Fallback {
    let names: Vec<_> = value
      .split(',')
      .map(str::trim)
      .filter(|name| !name.is_empty())
      .map(str::to_owned)
      .collect();
    for name in &names {
      if !config.providers.contains_key(name) {
        anyhow::bail!("provider {name} is not configured");
      }
    }
    config.fallback_providers = names;
  } else {
    let name = config
      .provider
      .as_ref()
      .ok_or_else(|| anyhow::anyhow!("save a named provider through Provider and model first"))?;
    let mut profile = config.providers[name].clone();
    match row {
      Row::Wire => {
        profile.wire_api = match value.as_str() {
          "chat" => WireApi::Chat,
          "responses" => WireApi::Responses,
          "messages" => WireApi::Messages,
          _ => anyhow::bail!("use chat, responses, or messages"),
        };
        if profile.wire_api != WireApi::Responses {
          profile.reasoning_effort = None;
        }
        if profile.wire_api != WireApi::Messages {
          profile.thinking_tokens = 0;
        }
      }
      Row::Reasoning => {
        profile.reasoning_effort = (!value.is_empty() && value != "default").then_some(value)
      }
      _ => {}
    }
    profile.validate(name)?;
    config.providers.insert(name.clone(), profile);
  }
  Ok(true)
}

fn step<T: Copy + PartialEq>(values: &[T], current: T, forward: bool) -> T {
  let index = values
    .iter()
    .position(|value| *value == current)
    .unwrap_or(0);
  let next = if forward {
    (index + 1) % values.len()
  } else {
    (index + values.len() - 1) % values.len()
  };
  values[next]
}

fn on_off(value: bool) -> String {
  if value { "on".into() } else { "off".into() }
}

/// The settings screen. Returns true when the user asked for provider setup.
pub async fn settings(config: &mut Config, headers: &HeaderCatalog) -> Result<bool> {
  let synapse_was_active = config.synapse_active();
  let names: Vec<String> = headers
    .headers
    .iter()
    .map(|header| header.name.clone())
    .collect();
  let mut terminal = enter_terminal()?;
  let result = run(&mut terminal, config, &names);
  leave_terminal(&mut terminal)?;
  let (configure, changed) = result?;
  if changed {
    match (synapse_was_active, config.synapse_active()) {
      (false, true) if synapse::binary(&config.synapse).is_some() => {
        synapse::connect(config, None, false).await?;
      }
      (true, false) => synapse::disconnect(config).await?,
      _ => config.save().await?,
    }
  }
  Ok(configure)
}

fn run(terminal: &mut Term, config: &mut Config, headers: &[String]) -> Result<(bool, bool)> {
  let mut selected = 0usize;
  let mut section = 0usize;
  let mut query = String::new();
  let mut searching = false;
  let mut changed = false;
  let installed = synapse::binary(&config.synapse).is_some();
  loop {
    let visible = rows(section, &query);
    selected = selected.min(visible.len().saturating_sub(1));
    terminal.draw(|frame| {
      render(
        frame,
        config,
        selected,
        installed,
        &visible,
        (section, &query, searching),
      )
    })?;
    let key = match event::read()? {
      InputEvent::Mouse(mouse) => {
        match mouse.kind {
          InputMouse::ScrollUp => selected = selected.saturating_sub(1),
          InputMouse::ScrollDown => selected = (selected + 1).min(visible.len().saturating_sub(1)),
          _ => {}
        };
        continue;
      }
      InputEvent::Key(key) if key.kind != KeyEventKind::Release => key,
      _ => continue,
    };
    if searching {
      match key.code {
        KeyCode::Esc => {
          query.clear();
          searching = false;
        }
        KeyCode::Enter => searching = false,
        KeyCode::Backspace => {
          query.pop();
          selected = 0;
        }
        KeyCode::Char(ch) => {
          query.push(ch);
          selected = 0;
        }
        _ => {}
      }
      continue;
    }
    match key.code {
      KeyCode::Char('/') => searching = true,
      KeyCode::Tab => {
        section = (section + 1) % SECTIONS.len();
        selected = 0;
      }
      KeyCode::BackTab => {
        section = (section + SECTIONS.len() - 1) % SECTIONS.len();
        selected = 0;
      }
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => {
        selected = (selected + 1).min(visible.len().saturating_sub(1))
      }
      KeyCode::Enter
      | KeyCode::Char(' ')
      | KeyCode::Right
      | KeyCode::Char('l')
      | KeyCode::Left
      | KeyCode::Char('h') => {
        let Some(row) = visible.get(selected).copied() else {
          continue;
        };
        match row {
          Row::Wire | Row::Reasoning | Row::Fallback => {
            changed |= provider_setting(terminal, config, row)?
          }
          Row::Binding(action) => changed |= binding(terminal, config, action)?,
          Row::Status => changed |= super::status::edit(terminal, &mut config.ui.status_line)?,
          _ => {
            if row.cycle(
              config,
              !matches!(key.code, KeyCode::Left | KeyCode::Char('h')),
              headers,
            ) {
              return Ok((true, changed));
            }
            changed = true;
          }
        }
      }
      KeyCode::Esc if !query.is_empty() => query.clear(),
      KeyCode::Esc | KeyCode::Char('q') => return Ok((false, changed)),
      _ => {}
    }
  }
}

fn render(
  frame: &mut Frame,
  config: &Config,
  selected: usize,
  installed: bool,
  visible: &[Row],
  filter: (usize, &str, bool),
) {
  let (section, query, searching) = filter;
  let [header, body, footer] = Layout::vertical([
    Constraint::Length(3),
    Constraint::Min(10),
    Constraint::Length(1),
  ])
  .areas(frame.area());
  super::render_header(
    frame,
    header,
    "Settings",
    &format!(
      "{} · / search{}{} · saved when you close",
      SECTIONS[section],
      if searching { ": " } else { " " },
      query
    ),
  );
  let [list, detail] = Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)])
    .spacing(2)
    .areas(body);
  let width = visible
    .iter()
    .map(|row| row.label().len())
    .max()
    .unwrap_or(20);
  let items = visible.iter().map(|row| {
    let value = row.value(config, installed);
    let tone = if value.starts_with("off") || value.contains("not installed") {
      MUTED
    } else if value.contains("needs") {
      YELLOW
    } else {
      ACTIVE
    };
    ListItem::new(Line::from(vec![
      Span::styled(
        format!("{:<width$}  ", row.label()),
        Style::default().fg(INK),
      ),
      Span::styled(value, Style::default().fg(tone)),
    ]))
  });
  let mut state = ListState::default().with_selected(Some(selected));
  frame.render_stateful_widget(
    List::new(items)
      .block(
        Block::default()
          .title(" Ainz ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(MUTED))
          .padding(Padding::horizontal(1)),
      )
      .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
    list,
    &mut state,
  );
  let Some(row) = visible.get(selected) else {
    return;
  };
  let (kind, description) = row.detail(config, installed);
  frame.render_widget(
    Paragraph::new(vec![
      Line::styled(
        row.label(),
        Style::default().fg(INK).add_modifier(Modifier::BOLD),
      ),
      Line::raw(""),
      Line::styled(kind, Style::default().fg(ACCENT)),
      Line::raw(""),
      Line::styled(description, Style::default().fg(MUTED)),
    ])
    .wrap(Wrap { trim: false })
    .block(
      Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .padding(Padding::new(2, 2, 1, 1)),
    ),
    detail,
  );
  super::render_footer(
    frame,
    footer,
    "Tab section · / search · ↑↓ move · Enter change · Esc close",
  );
}
