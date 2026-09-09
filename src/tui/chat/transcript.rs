use super::*;

pub(super) fn entry_lines(entry: &Entry, expanded: bool, width: usize) -> Vec<Line<'static>> {
  match &entry.tool {
    Some(tool) => call_lines(entry, tool, expanded),
    None => said_lines(entry, width),
  }
}

fn said_lines(entry: &Entry, width: usize) -> Vec<Line<'static>> {
  let (nick, color) = match entry.kind {
    EntryKind::User => ("<you>", CYAN),
    EntryKind::Assistant => ("<Ainz>", ACTIVE),
    EntryKind::System | EntryKind::Cancelled => ("***", YELLOW),
    EntryKind::Tool => ("tool", MAGENTA),
    EntryKind::Error => ("***", RED),
    EntryKind::Completed => ("***", ACTIVE),
  };
  let time = if width >= 32 {
    format!(" {} ", entry.at)
  } else {
    " ".into()
  };
  let prefix = Line::from(vec![
    Span::styled(time, Style::default().fg(MUTED)),
    Span::styled(
      format!("{nick:>6} "),
      Style::default().fg(color).add_modifier(Modifier::BOLD),
    ),
  ]);
  let options = termweave::Options {
    width,
    prefix: termweave::ratatui::from_line(&prefix),
    theme: termweave::Theme {
      text: termweave::ratatui::from_style(Style::default().fg(INK)),
      heading: termweave::Style {
        bold: true,
        ..Default::default()
      },
      code: termweave::ratatui::from_style(Style::default().fg(CYAN)),
      link: termweave::ratatui::from_style(
        Style::default().fg(CYAN).add_modifier(Modifier::UNDERLINED),
      ),
      marker: termweave::ratatui::from_style(Style::default().fg(MUTED)),
    },
  };
  let document = if matches!(entry.kind, EntryKind::Assistant) {
    match &entry.stream {
      Some(stream) => stream.snapshot(&options),
      None => termweave::render(&entry.text, &options),
    }
  } else {
    let style = Style::default().fg(if matches!(entry.kind, EntryKind::User | EntryKind::Tool) {
      INK
    } else {
      color
    });
    let body = entry
      .text
      .lines()
      .map(|text| termweave::Line::styled(text, termweave::ratatui::from_style(style)))
      .collect();
    termweave::wrap(body, &options)
  };
  document.ratatui().lines
}

/// A tool call, drawn as the command line it is: a mark for how it went, what ran, what it ran
/// on, and what it wrote underneath.
fn call_lines(entry: &Entry, tool: &ToolLine, expanded: bool) -> Vec<Line<'static>> {
  let (glyph, color) = tool.state.glyph();
  let mut head = vec![
    Span::styled(format!(" {}  ", entry.at), Style::default().fg(MUTED)),
    Span::styled(
      format!("{glyph} "),
      Style::default().fg(color).add_modifier(Modifier::BOLD),
    ),
    Span::styled(
      format!("{:<8}", clip(&tool.name, 8)),
      Style::default().fg(MAGENTA).add_modifier(Modifier::BOLD),
    ),
    Span::styled(tool.subject.clone(), Style::default().fg(INK)),
  ];
  if !tool.note.is_empty() {
    head.push(Span::styled(
      format!(" · {}", tool.note),
      Style::default().fg(MUTED),
    ));
  }
  let indent = "           ";
  let mut lines = vec![Line::from(head)];
  // while it runs, the last line it wrote; once it is over, the first few, or all of them
  let body: Vec<String> = match (&tool.live, &entry.detail, expanded) {
    (_, Some(detail), true) => detail.lines().map(str::to_string).collect(),
    (live, _, _) if !live.is_empty() => vec![live.clone()],
    // one line of output is already in the note beside the call
    (_, Some(detail), false)
      if detail
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > 1 =>
    {
      preview(detail)
    }
    (_, Some(_), false) => Vec::new(),
    _ => Vec::new(),
  };
  for line in body {
    lines.push(Line::from(vec![
      Span::raw(indent),
      Span::styled(
        if expanded { line } else { clip(&line, 160) },
        Style::default().fg(MUTED),
      ),
    ]));
  }
  lines
}

/// The first few lines of what a call wrote, and a count of what is not shown.
fn preview(detail: &str) -> Vec<String> {
  let mut lines = detail.lines().filter(|line| !line.trim().is_empty());
  let shown: Vec<String> = lines.by_ref().take(3).map(str::to_string).collect();
  match lines.count() {
    0 => shown,
    rest => [shown, vec![format!("… +{rest} lines · Ctrl+O expand")]].concat(),
  }
}
