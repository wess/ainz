use super::*;

impl HeaderArt {
  /// Fit bundled mascots to smaller terminals without replacing them with wordmarks.
  pub fn fitted_lines(&self, width: usize, height: usize) -> Option<Vec<Line<'static>>> {
    let lines = if self.width <= width && self.lines.len() <= height {
      self.lines.clone()
    } else if self.path.as_os_str().is_empty() && self.name == "mascot" {
      [
        include_str!("../../assets/mascot/compact.ans"),
        include_str!("../../assets/mascot/tiny.ans"),
        " .---. \n |o o| \n |[=]| \n /___\\ ",
        "(o_o)",
      ]
      .into_iter()
      .filter_map(|text| parse_ansi(text).ok())
      .find(|lines| lines.len() <= height && lines.iter().all(|line| line.width() <= width))
      .unwrap_or_default()
    } else {
      return None;
    };
    let canvas = lines.iter().map(Line::width).max().unwrap_or_default();
    Some(
      lines
        .into_iter()
        .map(|mut line| {
          line
            .spans
            .push(Span::raw(" ".repeat(canvas.saturating_sub(line.width()))));
          line.alignment(ratatui::layout::Alignment::Center)
        })
        .collect(),
    )
  }
}
