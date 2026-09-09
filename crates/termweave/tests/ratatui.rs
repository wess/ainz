#![cfg(feature = "ratatui")]

use ratatui_core::{
  buffer::Buffer,
  layout::Rect,
  style::{Color, Modifier, Style},
  text::Line,
  widgets::Widget,
};
use termweave::{Options, ratatui, render};

#[test]
fn adapter_renders_the_same_cells_and_styles() {
  let document = render(
    "# Title\n\n**bold** and `code`",
    &Options {
      width: 40,
      ..Options::default()
    },
  );
  let text = document.ratatui();
  let area = Rect::new(0, 0, 40, document.lines.len() as u16);
  let mut buffer = Buffer::empty(area);
  text.render(area, &mut buffer);
  for (y, line) in document.lines.iter().enumerate() {
    let cells: String = (0..area.width)
      .map(|x| buffer[(x, y as u16)].symbol())
      .collect();
    assert_eq!(cells.trim_end(), line.text().trim_end());
  }
  assert!(buffer[(0, 0)].modifier.contains(Modifier::BOLD));
  assert!(buffer[(0, 2)].modifier.contains(Modifier::BOLD));
}

#[test]
fn conversion_preserves_inherited_style_and_prefix_colors() {
  let style = Style::default()
    .fg(Color::Rgb(10, 20, 30))
    .bg(Color::Indexed(4))
    .add_modifier(Modifier::BOLD | Modifier::ITALIC);
  let original = Line::raw("<nick> ").style(style);
  let converted = ratatui::from_line(&original);
  assert_eq!(converted.width(), 7);
  assert_eq!(ratatui::line(&converted).spans[0].style, style);
}
