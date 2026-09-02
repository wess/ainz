use ainz::{Config, HeaderCatalog, MemoryBackend, PermissionMode, synapse};
use anyhow::Result;
use crossterm::event::{self, Event as InputEvent, KeyCode, KeyEventKind};
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
  Permissions,
  Memory,
  RecallOnStart,
  RememberOnCompact,
  Teach,
  Synapse,
  Mesh,
  Roster,
  Header,
}

const ROWS: [Row; 10] = [
  Row::Provider,
  Row::Permissions,
  Row::Memory,
  Row::RecallOnStart,
  Row::RememberOnCompact,
  Row::Teach,
  Row::Synapse,
  Row::Mesh,
  Row::Roster,
  Row::Header,
];

impl Row {
  fn label(self) -> &'static str {
    match self {
      Self::Provider => "Provider and model",
      Self::Permissions => "Permissions",
      Self::Memory => "Memory",
      Self::RecallOnStart => "Recall at session start",
      Self::RememberOnCompact => "Remember before compaction",
      Self::Teach => "Self-improvement",
      Self::Synapse => "Synapse",
      Self::Mesh => "Agent mesh",
      Self::Roster => "Agent roster",
      Self::Header => "Header art",
    }
  }

  fn value(self, config: &Config, installed: bool) -> String {
    match self {
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
    }
  }

  fn detail(self, config: &Config, installed: bool) -> (String, String) {
    match self {
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
      Self::Synapse => (
        if installed {
          "Installed".into()
        } else {
          "Not installed".into()
        },
        format!(
          "{} Turning this on registers Synapse as a tool server, loads its SOUL.md guidance, \
           and lets memory and skills live there instead of in one tool. Optional: Ainz runs \
           the same without it. {}",
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
      Self::Provider => return true,
      Self::Permissions => {
        let modes = [
          PermissionMode::Ask,
          PermissionMode::Auto,
          PermissionMode::ReadOnly,
        ];
        config.permissions = step(&modes, config.permissions, forward);
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
    config.save().await?;
  }
  Ok(configure)
}

fn run(terminal: &mut Term, config: &mut Config, headers: &[String]) -> Result<(bool, bool)> {
  let mut selected = 0;
  let mut changed = false;
  loop {
    let installed = synapse::binary(&config.synapse).is_some();
    terminal.draw(|frame| render(frame, config, selected, installed))?;
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(ROWS.len() - 1),
      KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
        if ROWS[selected].cycle(config, true, headers) {
          return Ok((true, changed));
        }
        changed = true;
      }
      KeyCode::Left | KeyCode::Char('h') => {
        if ROWS[selected].cycle(config, false, headers) {
          return Ok((true, changed));
        }
        changed = true;
      }
      KeyCode::Esc | KeyCode::Char('q') => return Ok((false, changed)),
      _ => {}
    }
  }
}

fn render(frame: &mut Frame, config: &Config, selected: usize, installed: bool) {
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
    "Changes are saved as you make them",
  );
  let [list, detail] = Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)])
    .spacing(2)
    .areas(body);
  let width = ROWS.iter().map(|row| row.label().len()).max().unwrap_or(20);
  let items = ROWS.iter().map(|row| {
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
  let (kind, description) = ROWS[selected].detail(config, installed);
  frame.render_widget(
    Paragraph::new(vec![
      Line::styled(
        ROWS[selected].label(),
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
  super::render_footer(frame, footer, "↑↓ move   enter/←→ change   esc close");
}
