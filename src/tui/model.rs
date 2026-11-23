use ainz::{
  Config, Session,
  routing::{ModelMode, RoutingStrategy, model_mode, models},
};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::{
  layout::{Constraint, Layout, Rect},
  style::{Modifier, Style},
  widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use super::{ACTIVE, BLUE, INK, MUTED, Term};

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Pick {
  Mode(ModelMode),
  Providers,
}

struct Row {
  pick: Pick,
  label: String,
  search: String,
}

impl Row {
  fn new(pick: Pick, label: String) -> Self {
    Self {
      search: label.to_lowercase(),
      label,
      pick,
    }
  }
}

pub(super) fn pick(
  terminal: &mut Term,
  config: &Config,
  session: &Session,
) -> Result<Option<Pick>> {
  let current = Pick::Mode(model_mode(config, session));
  let mut rows = Vec::new();
  if config.boss.enabled {
    rows.extend(RoutingStrategy::ALL.map(|strategy| {
      Row::new(
        Pick::Mode(ModelMode::Auto { strategy }),
        format!("Auto · {}", strategy.label()),
      )
    }));
  }
  rows.extend(models(config).into_keys().map(|target| {
    let label = target.to_string();
    Row::new(Pick::Mode(ModelMode::Fixed { target }), label)
  }));
  rows.push(Row::new(Pick::Providers, "Add or manage providers…".into()));
  let mut search = String::new();
  let mut list =
    ListState::default().with_selected(rows.iter().position(|row| row.pick == current).or(Some(0)));
  loop {
    let query = search.to_lowercase();
    let visible: Vec<_> = rows
      .iter()
      .filter(|row| {
        query
          .split_whitespace()
          .all(|word| row.search.contains(word))
      })
      .collect();
    let mut body = Rect::ZERO;
    terminal.draw(|frame| {
      let [head, area, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
      ])
      .areas(frame.area());
      body = area;
      frame.render_widget(
        Paragraph::new(format!("Choose a model\nSearch: {search}")).style(Style::default().fg(INK)),
        head,
      );
      let entries = visible.iter().map(|row| {
        let text = if row.pick == current {
          format!("• {}", row.label)
        } else {
          format!("  {}", row.label)
        };
        let style = if row.pick == current {
          Style::default().fg(ACTIVE)
        } else {
          Style::default().fg(INK)
        };
        ListItem::new(text).style(style)
      });
      let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BLUE));
      if visible.is_empty() {
        frame.render_widget(
          Paragraph::new("No matching models.")
            .block(block)
            .style(Style::default().fg(MUTED)),
          area,
        );
      } else {
        frame.render_stateful_widget(
          List::new(entries)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
          area,
          &mut list,
        );
      }
      frame.render_widget(
        Paragraph::new("↑↓ select · Enter apply · Esc cancel").style(Style::default().fg(MUTED)),
        footer,
      );
    })?;
    match event::read()? {
      Event::Paste(text) => {
        search.extend(text.chars().filter(|character| !character.is_control()));
        list.select(Some(0));
      }
      Event::Mouse(mouse) if !visible.is_empty() => match mouse.kind {
        MouseEventKind::ScrollUp => list.select_previous(),
        MouseEventKind::ScrollDown => list.select_next(),
        MouseEventKind::Down(_)
          if mouse.row > body.y
            && mouse.row < body.bottom().saturating_sub(1)
            && mouse.column >= body.x
            && mouse.column < body.right() =>
        {
          let index = list.offset() + usize::from(mouse.row - body.y - 1);
          if let Some(row) = visible.get(index) {
            return Ok(Some(row.pick.clone()));
          }
        }
        _ => {}
      },
      Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
        KeyCode::Esc => return Ok(None),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
        KeyCode::Enter => {
          if let Some(row) = list.selected().and_then(|index| visible.get(index)) {
            return Ok(Some(row.pick.clone()));
          }
        }
        KeyCode::Up if !visible.is_empty() => list.select_previous(),
        KeyCode::Down if !visible.is_empty() => {
          let index = list.selected().unwrap_or(0);
          list.select(Some((index + 1).min(visible.len() - 1)));
        }
        KeyCode::Backspace => {
          search.pop();
          list.select(Some(0));
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
          search.clear();
          list.select(Some(0));
        }
        KeyCode::Char(character)
          if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
          search.push(character);
          list.select(Some(0));
        }
        _ => {}
      },
      _ => {}
    }
  }
}
