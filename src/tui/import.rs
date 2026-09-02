use std::path::Path;

use ainz::{Candidate, Config, import as importer};
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

/// Offers what other tools on this machine already have, and copies over what is chosen.
pub(crate) async fn import(workspace: &Path, config: &Config) -> Result<()> {
  let memory = crate::app::memory_store(workspace, config).await?;
  let found = importer::discover(workspace, &memory).await?;
  let mut terminal = enter_terminal()?;
  let result = run(&mut terminal, &found, &memory).await;
  leave_terminal(&mut terminal)?;
  result
}

async fn run(terminal: &mut Term, found: &[Candidate], memory: &ainz::MemoryStore) -> Result<()> {
  // anything Ainz already reads starts unticked; the rest is what the screen is for
  let mut chosen: Vec<bool> = found.iter().map(|candidate| !candidate.present).collect();
  let Some(picked) = select(terminal, found, &mut chosen)? else {
    return Ok(());
  };
  if picked.is_empty() {
    return Ok(());
  }
  terminal.draw(|frame| {
    let area = super::centered(frame.area(), frame.area().width.min(60), 5);
    frame.render_widget(
      Paragraph::new("Importing…")
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(ACCENT))
        .block(
          Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MUTED))
            .padding(Padding::vertical(1)),
        ),
      area,
    );
  })?;
  let done = importer::import(&picked, memory).await?;
  summary(terminal, &done)
}

fn select(
  terminal: &mut Term,
  found: &[Candidate],
  chosen: &mut [bool],
) -> Result<Option<Vec<Candidate>>> {
  if found.is_empty() {
    return empty(terminal).map(|()| None);
  }
  let mut selected = 0;
  loop {
    terminal.draw(|frame| render(frame, found, chosen, selected))?;
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(found.len() - 1),
      KeyCode::Char(' ') => chosen[selected] = !chosen[selected],
      KeyCode::Char('a') => {
        let all = chosen.iter().all(|value| *value);
        for (index, value) in chosen.iter_mut().enumerate() {
          *value = !all && !found[index].present;
        }
      }
      KeyCode::Enter => {
        return Ok(Some(
          found
            .iter()
            .zip(chosen.iter())
            .filter(|(_, picked)| **picked)
            .map(|(candidate, _)| candidate.clone())
            .collect(),
        ));
      }
      KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
      _ => {}
    }
  }
}

fn empty(terminal: &mut Term) -> Result<()> {
  loop {
    terminal.draw(|frame| {
      let area = super::centered(frame.area(), frame.area().width.min(70), 7);
      frame.render_widget(
        Paragraph::new(vec![
          Line::styled(
            "Nothing to import",
            Style::default().fg(INK).add_modifier(Modifier::BOLD),
          ),
          Line::raw(""),
          Line::styled(
            "Ainz already reads the skills, commands, and instructions the other tools on \
             this machine keep, and found no tool servers it does not have.",
            Style::default().fg(MUTED),
          ),
        ])
        .wrap(Wrap { trim: false })
        .block(
          Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MUTED))
            .padding(Padding::new(2, 2, 1, 1)),
        ),
        area,
      );
    })?;
    if let InputEvent::Key(key) = event::read()?
      && key.kind != KeyEventKind::Release
    {
      return Ok(());
    }
  }
}

fn summary(terminal: &mut Term, done: &[String]) -> Result<()> {
  loop {
    terminal.draw(|frame| {
      let [body, footer] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(frame.area());
      let lines: Vec<Line> = done
        .iter()
        .map(|line| Line::styled(line.clone(), Style::default().fg(ACTIVE)))
        .collect();
      frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
          Block::default()
            .title(" Imported ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MUTED))
            .padding(Padding::new(2, 2, 1, 1)),
        ),
        body,
      );
      super::render_footer(frame, footer, "any key returns to the session");
    })?;
    if let InputEvent::Key(key) = event::read()?
      && key.kind != KeyEventKind::Release
    {
      return Ok(());
    }
  }
}

fn render(frame: &mut Frame, found: &[Candidate], chosen: &[bool], selected: usize) {
  let [header, body, footer] = Layout::vertical([
    Constraint::Length(3),
    Constraint::Min(10),
    Constraint::Length(1),
  ])
  .areas(frame.area());
  let count = chosen.iter().filter(|picked| **picked).count();
  super::render_header(
    frame,
    header,
    "Import",
    &format!("{count} of {} selected", found.len()),
  );
  let [list, detail] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
    .spacing(2)
    .areas(body);
  let width = found
    .iter()
    .map(|candidate| candidate.name.len())
    .max()
    .unwrap_or(16)
    .min(28);
  let items = found.iter().zip(chosen.iter()).map(|(candidate, picked)| {
    let (mark, tone) = match (picked, candidate.present) {
      (true, _) => ("[x] ", ACTIVE),
      (false, true) => ("[·] ", MUTED),
      (false, false) => ("[ ] ", INK),
    };
    ListItem::new(Line::from(vec![
      Span::styled(mark, Style::default().fg(tone)),
      Span::styled(
        format!("{:<width$}  ", candidate.name),
        Style::default().fg(tone),
      ),
      Span::styled(candidate.kind.label(), Style::default().fg(MUTED)),
    ]))
  });
  let mut state = ListState::default().with_selected(Some(selected));
  frame.render_stateful_widget(
    List::new(items)
      .block(
        Block::default()
          .title(" Found on this machine ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(MUTED))
          .padding(Padding::horizontal(1)),
      )
      .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
    list,
    &mut state,
  );

  let candidate = &found[selected];
  let mut lines = vec![
    Line::styled(
      candidate.name.clone(),
      Style::default().fg(INK).add_modifier(Modifier::BOLD),
    ),
    Line::raw(""),
    Line::styled(candidate.origin.clone(), Style::default().fg(ACCENT)),
    Line::raw(""),
    Line::styled(candidate.detail.clone(), Style::default().fg(MUTED)),
  ];
  if let Ok(target) = candidate.target() {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
      format!("Copied into {target}"),
      Style::default().fg(MUTED),
    ));
  }
  if candidate.present {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
      "Ainz already has this; importing it again would change nothing.",
      Style::default().fg(MUTED),
    ));
  }
  if candidate.secrets {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
      "This entry holds a token or password inline rather than naming an environment \
       variable, so importing copies the secret into the Ainz profile.",
      Style::default().fg(YELLOW),
    ));
  }
  frame.render_widget(
    Paragraph::new(lines).wrap(Wrap { trim: false }).block(
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
    "↑↓ move   space select   a all/none   enter import   esc cancel",
  );
}
