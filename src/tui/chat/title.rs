use super::*;

impl AgentView {
  pub(super) fn refresh_topic(&mut self) {
    self.topic = self
      .entries
      .iter()
      .rev()
      .find(|entry| matches!(entry.kind, EntryKind::User))
      .map(|entry| excerpt(&entry.text))
      .unwrap_or_default();
  }
}

pub(super) fn excerpt(text: &str) -> String {
  let text: String = text
    .chars()
    .take(512)
    .map(|ch| if ch.is_control() { ' ' } else { ch })
    .collect();
  fit(&text.split_whitespace().collect::<Vec<_>>().join(" "), 120)
}

fn fit(text: &str, width: usize) -> String {
  if Line::raw(text).width() <= width {
    return text.to_string();
  }
  if width == 0 {
    return String::new();
  }
  let span = Span::raw(text);
  let mut result = String::new();
  let mut used = 0;
  for grapheme in span.styled_graphemes(Style::default()) {
    let size = Line::raw(grapheme.symbol).width();
    if used + size >= width {
      break;
    }
    result.push_str(grapheme.symbol);
    used += size;
  }
  result.push('…');
  result
}

pub(super) fn render(
  frame: &mut Frame,
  area: Rect,
  state: &ChatState,
  session: Uuid,
  workspace: &std::path::Path,
) {
  let view = state.active_view();
  let channel = fit(
    &excerpt(&state.channel()),
    (area.width as usize / 3).min(24),
  );
  let topic = if view.topic.is_empty() {
    "Ready"
  } else {
    &view.topic
  };
  let tool = view
    .live
    .values()
    .filter(|_| matches!(view.state, AgentState::Running))
    .max_by_key(|(index, _)| index)
    .and_then(|(index, _)| view.entries.get(*index))
    .and_then(|entry| entry.tool.as_ref());
  let topic = match tool {
    Some(tool) => format!("{} {} · {topic}", tool.name, tool.subject),
    None => topic.to_string(),
  };
  let context = if area.width >= 100 {
    let root = workspace
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("workspace");
    format!(
      " {} · {} ",
      fit(&excerpt(root), 18),
      &session.to_string()[..8]
    )
  } else {
    String::new()
  };
  let prefix = format!(" {channel} · ");
  let width =
    (area.width as usize).saturating_sub(Line::raw(&prefix).width() + Line::raw(&context).width());
  let topic = fit(&excerpt(&topic), width);
  let padding = width.saturating_sub(Line::raw(&topic).width());
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(
        prefix,
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
      ),
      Span::styled(topic, Style::default().fg(INK)),
      Span::raw(" ".repeat(padding)),
      Span::styled(context, Style::default().fg(MUTED)),
    ])),
    area,
  );
}
