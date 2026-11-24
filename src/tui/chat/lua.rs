//! The interface's side of the person's scripts: what they are told about, what they asked
//! for, and the columns and bars they draw. Every function here returns at once when no
//! script is loaded.

use std::{collections::VecDeque, sync::Arc};

use ainz::{
  Event,
  script::{self, Color as Tint, Effect, Key, Level, Panel, Script, Side, View},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
  Frame,
  layout::{Constraint, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph},
};
use serde_json::json;

use super::{BLUE, ChatState, Entry, EntryKind};

pub(super) fn script() -> Option<Arc<Script>> {
  crate::scripting::current()
}

/// What scripts asked of the interface that it has not done yet.
#[derive(Default)]
pub(super) struct Scripted {
  /// Text to send as if typed, one at a time when the prompt is free.
  pub outbox: VecDeque<String>,
  pub rebuild: bool,
  pub quit: bool,
  /// The prompt being sent came from a script: it stays out of history, and the person's
  /// own draft goes back in the line afterwards.
  pub synthetic: bool,
  pub stash: Option<String>,
  /// The first key of a chord that a script's keymap may finish.
  pub prefix: Option<KeyEvent>,
}

/// Agent events a script may listen for. Streaming text and tool output are left out: they
/// arrive a piece at a time, and a handler on each would cost more than it could show.
pub(super) fn event(event: &Event) {
  let name = match event {
    Event::ModelSelected { .. } => "model_selected",
    Event::ToolStart { .. } => "tool_start",
    Event::ToolEnd { .. } => "tool_end",
    Event::Notice { .. } => "notice",
    Event::Error { .. } => "error",
    Event::Compaction { .. } => "compaction",
    Event::Context { .. } => "context",
    Event::Todos { .. } => "todos",
    Event::Steering { .. } => "steering",
    Event::Cancelled => "cancelled",
    Event::SubagentStart { .. } => "subagent_start",
    Event::SubagentEnd { .. } => "subagent_end",
    Event::WorkerUpdate { .. } => "worker_update",
    _ => return,
  };
  let Some(script) = script() else {
    return;
  };
  if !script.handles(name) {
    return;
  }
  let payload = match event {
    Event::ToolStart { call } => json!({
      "id": call.id,
      "name": call.name,
      "arguments": call.arguments,
      "subject": ainz::agent::subject(&call.arguments),
    }),
    Event::ToolEnd { id, output, error } => json!({
      "id": id,
      "error": error,
      "output": ainz::tool::truncate(output.clone(), 4096),
    }),
    other => serde_json::to_value(other).unwrap_or_default(),
  };
  script.emit(name, &payload);
}

/// A run that ended, for `turn_end` handlers.
pub(super) fn turn_end(result: &anyhow::Result<String>, cancelled: bool) {
  let Some(script) = script() else {
    return;
  };
  let payload = match result {
    Ok(text) => json!({ "ok": !cancelled, "cancelled": cancelled, "text": text }),
    Err(error) => json!({ "ok": false, "cancelled": cancelled, "error": format!("{error:#}") }),
  };
  script.emit("turn_end", &payload);
}

pub(super) fn emit(name: &str, payload: serde_json::Value) {
  if let Some(script) = script() {
    script.emit(name, &payload);
  }
}

/// The interface as scripts see it.
pub(super) fn view(state: &ChatState, config: &ainz::Config) -> View {
  use ainz::script::{Agents, Context, Count, Tokens};
  let active = state.active_view();
  let running = state
    .agents
    .values()
    .filter(|view| matches!(view.state, super::AgentState::Running))
    .count();
  let done = active
    .todos
    .iter()
    .filter(|todo| todo.status == ainz::TodoStatus::Done)
    .count();
  let (width, height) = state.size.get();
  let (provider, model) = active.model_label(config);
  View {
    provider: provider.into(),
    model,
    mode: match config.yeet {
      true => "yeet".into(),
      false => super::permission_name(config.permissions).into(),
    },
    busy: state.busy(),
    activity: match (state.approval.is_some(), state.busy()) {
      (true, _) => "needs approval",
      (false, true) => "running",
      (false, false) => "ready",
    }
    .into(),
    input: state.input.as_str().to_string(),
    tokens: Tokens {
      input: active.usage.input_tokens,
      output: active.usage.output_tokens,
      total: active
        .usage
        .input_tokens
        .saturating_add(active.usage.output_tokens),
    },
    cost: active.usage.cost_usd,
    context: active.context.map(|(used, limit)| Context {
      used,
      limit,
      percent: (used * 100).checked_div(limit).unwrap_or(0).min(100),
    }),
    agents: Agents {
      running,
      total: state.agents.len(),
    },
    todos: Count {
      done,
      total: active.todos.len(),
    },
    branch: state.branch.clone(),
    session: state.session_name.clone(),
    workspace: state.workspace.display().to_string(),
    width,
    height,
  }
}

/// Tells scripts what the interface looks like now, for `ainz.state()` and the status items.
pub(super) fn describe(view: View) {
  if let Some(script) = script() {
    script.describe(view);
  }
}

/// Carries out what scripts asked for. True when something changed on screen.
pub(super) fn drain(state: &mut ChatState) -> bool {
  let Some(script) = script() else {
    return false;
  };
  let mut changed = script.take_redraw();
  for effect in script.effects() {
    changed = true;
    match effect {
      Effect::Notify { text, level } => {
        let kind = match level {
          Level::Error => EntryKind::Error,
          Level::Info | Level::Warn => EntryKind::System,
        };
        state.primary.entries.push(Entry::new(kind, text));
      }
      Effect::Send(text) => state.scripted.outbox.push_back(text),
      Effect::SetInput(text) => state.input.set(text),
      Effect::Insert(text) => state.input.insert_str(&text),
      Effect::Highlight { role, color } => {
        let Some(color) = paint(&color) else {
          continue;
        };
        if let Err(error) = state.theme.set(&role, color) {
          state
            .primary
            .entries
            .push(Entry::new(EntryKind::Error, format!("lua: {error:#}")));
        }
      }
      Effect::Rebuild => state.scripted.rebuild = true,
      Effect::Quit => state.scripted.quit = true,
    }
  }
  changed
}

/// A key a script mapped. Single keys answer before the built-in shortcuts, so a mapping can
/// replace one; a chord's second key answers before the built-ins see it.
pub(super) fn press(state: &mut ChatState, key: KeyEvent) -> bool {
  let Some(script) = script() else {
    return false;
  };
  let key = ainz::keybind::normalize(key);
  let maps = script.keymaps();
  let parsed = maps
    .iter()
    .filter_map(|(keys, target, _)| Some((ainz::keybind::parse(keys).ok()?, target)));
  let mut hit = None;
  if let Some(first) = state.scripted.prefix.take() {
    hit = parsed
      .clone()
      .find(|(keys, _)| keys.as_slice() == [first, key])
      .map(|(_, target)| target.clone());
  }
  if hit.is_none() && commanding(key) {
    hit = parsed
      .clone()
      .find(|(keys, _)| keys.as_slice() == [key])
      .map(|(_, target)| target.clone());
  }
  let Some(target) = hit else {
    return false;
  };
  match target {
    Key::Run(id) => script.press(id),
    Key::Send(text) => state.scripted.outbox.push_back(text),
  }
  true
}

/// Whether a script's chord starts with this key, so it waits for the next one.
pub(super) fn starts_chord(key: KeyEvent) -> bool {
  let Some(script) = script() else {
    return false;
  };
  let key = ainz::keybind::normalize(key);
  commanding(key)
    && script.keymaps().iter().any(|(keys, ..)| {
      ainz::keybind::parse(keys).is_ok_and(|keys| keys.len() == 2 && keys[0] == key)
    })
}

/// Only a key that cannot be typed starts a mapping, so a script never takes a letter from the
/// prompt.
fn commanding(key: KeyEvent) -> bool {
  key
    .modifiers
    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    || matches!(
      key.code,
      KeyCode::F(_) | KeyCode::PageUp | KeyCode::PageDown
    )
}

/// The panels scripts opened, by side, left out when the terminal is too narrow for them and
/// a transcript beside them.
pub(super) fn panels(width: u16) -> (Vec<Panel>, Vec<Panel>) {
  let Some(script) = script() else {
    return (Vec::new(), Vec::new());
  };
  let (left, right): (Vec<Panel>, Vec<Panel>) = script
    .panels()
    .into_iter()
    .partition(|panel| panel.side == Side::Left);
  let wide = |panels: &[Panel]| panels.iter().map(|panel| panel.width).max().unwrap_or(0);
  match width.saturating_sub(wide(&left) + wide(&right)) >= 40 {
    true => (left, right),
    false if width.saturating_sub(wide(&right)) >= 40 => (Vec::new(), right),
    false => (Vec::new(), Vec::new()),
  }
}

pub(super) fn column(panels: &[Panel]) -> u16 {
  panels.iter().map(|panel| panel.width).max().unwrap_or(0)
}

/// A column of panels, stacked top to bottom; `rest` is what is left under them, for the
/// roster to take.
pub(super) fn render_column(frame: &mut Frame, area: Rect, panels: &[Panel], rest: bool) -> Rect {
  let mut constraints: Vec<Constraint> = panels
    .iter()
    .map(|panel| match panel.height {
      Some(rows) => Constraint::Length(rows.saturating_add(1)),
      None => Constraint::Fill(1),
    })
    .collect();
  if rest {
    constraints.push(Constraint::Fill(1));
  }
  let areas = Layout::vertical(constraints).split(area);
  // the transcript draws its own rule on the left, so a left column needs none of its own
  let borders = match panels.first().map(|panel| panel.side) {
    Some(Side::Left) => Borders::NONE,
    _ => Borders::LEFT,
  };
  for (panel, area) in panels.iter().zip(areas.iter()) {
    let rows = usize::from(area.height.saturating_sub(1));
    let skip = match panel.follow {
      true => panel.lines.len().saturating_sub(rows),
      false => 0,
    };
    let lines: Vec<Line> = panel
      .lines
      .iter()
      .skip(skip)
      .take(rows)
      .map(|line| styled(line, Style::default()))
      .collect();
    let title = match panel.title.is_empty() {
      true => String::new(),
      false => format!(" {} ", panel.title),
    };
    frame.render_widget(
      Paragraph::new(lines).block(
        Block::default()
          .title(title)
          .borders(borders)
          .border_style(Style::default().fg(BLUE)),
      ),
      *area,
    );
  }
  match rest {
    true => areas.last().copied().unwrap_or_default(),
    false => Rect::default(),
  }
}

/// A script's line as the terminal draws it, over `base` for anything it leaves unstyled.
pub(super) fn styled(line: &script::Line, base: Style) -> Line<'static> {
  Line::from(
    line
      .iter()
      .map(|segment| {
        let mut style = base;
        if let Some(fg) = segment.style.fg.as_ref().and_then(paint) {
          style = style.fg(fg);
        }
        if let Some(bg) = segment.style.bg.as_ref().and_then(paint) {
          style = style.bg(bg);
        }
        for (on, modifier) in [
          (segment.style.bold, Modifier::BOLD),
          (segment.style.italic, Modifier::ITALIC),
          (segment.style.underline, Modifier::UNDERLINED),
          (segment.style.reverse, Modifier::REVERSED),
        ] {
          if on {
            style = style.add_modifier(modifier);
          }
        }
        Span::styled(segment.text.clone(), style)
      })
      .collect::<Vec<_>>(),
  )
}

/// A theme role is drawn in the colour the theme pass recognises for it, so `/theme` recolours
/// what a script drew the same way it recolours everything else.
fn paint(tint: &Tint) -> Option<Color> {
  Some(match tint {
    Tint::Ansi(index) => [
      Color::Black,
      Color::Red,
      Color::Green,
      Color::Yellow,
      Color::Blue,
      Color::Magenta,
      Color::Cyan,
      Color::Gray,
      Color::DarkGray,
      Color::LightRed,
      Color::LightGreen,
      Color::LightYellow,
      Color::LightBlue,
      Color::LightMagenta,
      Color::LightCyan,
      Color::White,
    ][usize::from(*index).min(15)],
    Tint::Rgb(red, green, blue) => Color::Rgb(*red, *green, *blue),
    Tint::Role(role) => ainz::theme::COLORS
      .iter()
      .find(|(name, _)| name == role)
      .map(|(_, color)| *color)?,
  })
}

/// `/lua`, `/source`, and `/trust`, and any command a script defined. True when it was one of
/// them.
pub(super) fn command(state: &mut ChatState, input: &str, config: &mut ainz::Config) -> bool {
  let input = input.trim();
  let Some(rest) = input.strip_prefix('/') else {
    return false;
  };
  let (name, args) = rest.split_once(' ').unwrap_or((rest, ""));
  let note =
    |state: &mut ChatState, kind, text: String| state.primary.entries.push(Entry::new(kind, text));
  match name {
    "lua" if args.trim().is_empty() => {
      let text = match script() {
        None => "no scripts loaded; write init.lua in the config directory, then /source".into(),
        Some(script) => {
          let files: Vec<String> = script
            .files()
            .iter()
            .map(|path| format!("  {}", path.display()))
            .collect();
          format!(
            "lua: {} file(s), {} keymap(s), {} command(s), {} panel(s)\n{}",
            files.len(),
            script.keymaps().len(),
            script.commands().len(),
            script.panels().len(),
            files.join("\n")
          )
        }
      };
      note(state, EntryKind::System, text);
    }
    "lua" => match script() {
      None => note(
        state,
        EntryKind::Error,
        "no scripts loaded; /source starts them".into(),
      ),
      Some(script) => match script.run(args) {
        Ok(shown) if shown.is_empty() => {}
        Ok(shown) => note(state, EntryKind::System, shown),
        Err(error) => note(state, EntryKind::Error, format!("lua: {error:#}")),
      },
    },
    "source" => {
      let workspace = state.workspace.clone();
      let notes = crate::scripting::reload(&workspace, config);
      let text = match notes.is_empty() {
        true => "scripts sourced".to_string(),
        false => notes.join("\n"),
      };
      note(state, EntryKind::System, text);
      state.scripted.rebuild = true;
    }
    "trust" => match crate::scripting::trust(&state.workspace) {
      Ok(path) => {
        note(
          state,
          EntryKind::System,
          format!("trusted {}; running it now", path.display()),
        );
        let workspace = state.workspace.clone();
        for text in crate::scripting::reload(&workspace, config) {
          note(state, EntryKind::System, text);
        }
        state.scripted.rebuild = true;
      }
      Err(error) => note(state, EntryKind::Error, format!("{error:#}")),
    },
    _ => return script().is_some_and(|script| script.command(name, args.trim())),
  }
  true
}

/// The commands scripts added, for the palette.
pub(super) fn commands() -> Vec<(String, String)> {
  script()
    .map(|script| {
      script
        .commands()
        .into_iter()
        .map(|command| (command.name, command.desc))
        .collect()
    })
    .unwrap_or_default()
}
