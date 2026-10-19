use ainz::config::StatusItem;

pub(crate) const DEFAULT: [StatusItem; 10] = [
  StatusItem::Attention,
  StatusItem::Provider,
  StatusItem::Model,
  StatusItem::Mode,
  StatusItem::Memory,
  StatusItem::Tokens,
  StatusItem::Context,
  StatusItem::Todos,
  StatusItem::Agents,
  StatusItem::Activity,
];

pub(crate) struct Editor {
  pub(crate) rows: Vec<(StatusItem, bool)>,
  pub(crate) index: usize,
}

impl Editor {
  pub(crate) fn new(items: Option<&[StatusItem]>) -> Self {
    let selected = items.unwrap_or(&DEFAULT);
    let mut rows = Vec::new();
    for item in selected {
      if !rows.iter().any(|(row, _)| row == item) {
        rows.push((*item, true));
      }
    }
    for item in StatusItem::ALL {
      if !selected.contains(&item) {
        rows.push((item, false));
      }
    }
    Self { rows, index: 0 }
  }

  pub(crate) fn selected(&self) -> Vec<StatusItem> {
    self
      .rows
      .iter()
      .filter(|(_, on)| *on)
      .map(|(item, _)| *item)
      .collect()
  }

  pub(crate) fn move_by(&mut self, delta: isize, reorder: bool) {
    let next = self
      .index
      .saturating_add_signed(delta)
      .min(self.rows.len() - 1);
    if reorder {
      self.rows.swap(self.index, next);
    }
    self.index = next;
  }
}

use super::{BLUE, INK, MUTED, Term};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
  layout::{Constraint, Layout},
  style::{Modifier, Style},
  text::Line,
  widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

pub(crate) fn edit(terminal: &mut Term, items: &mut Option<Vec<StatusItem>>) -> Result<bool> {
  let mut editor = Editor::new(items.as_deref());
  loop {
    terminal.draw(|frame| {
      let [head, body, footer] = Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(2)]).areas(frame.area());
      frame.render_widget(Paragraph::new("Status bar · fields appear in this order\nSpace toggles · Shift+↑↓ reorders · Enter saves · R resets · Esc cancels").style(Style::default().fg(INK)), head);
      let rows = editor.rows.iter().map(|(item, enabled)| ListItem::new(format!("[{}] {}", if *enabled { "x" } else { " " }, item.label())));
      frame.render_stateful_widget(List::new(rows).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(BLUE))).highlight_style(Style::default().add_modifier(Modifier::REVERSED)), body, &mut ListState::default().with_selected(Some(editor.index)));
      frame.render_widget(Paragraph::new(Line::raw(format!("Preview: {}", editor.selected().iter().map(|item| item.label()).collect::<Vec<_>>().join(" │ ")))).style(Style::default().fg(MUTED)), footer);
    })?;
    let Event::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Up => editor.move_by(-1, key.modifiers.contains(KeyModifiers::SHIFT)),
      KeyCode::Down => editor.move_by(1, key.modifiers.contains(KeyModifiers::SHIFT)),
      KeyCode::Char(' ') => editor.rows[editor.index].1 = !editor.rows[editor.index].1,
      KeyCode::Char('r' | 'R') => {
        *items = None;
        return Ok(true);
      }
      KeyCode::Enter => {
        *items = Some(editor.selected());
        return Ok(true);
      }
      KeyCode::Esc => return Ok(false),
      _ => {}
    }
  }
}
