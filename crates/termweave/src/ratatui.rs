//! Optional conversions to and from Ratatui's owned text primitives.
use ratatui_core::{
  style::{Color as TerminalColor, Modifier, Style as TerminalStyle},
  text::{Line as TerminalLine, Span as TerminalSpan, Text},
};

use crate::{Color, Document, Line, Span, Style, text::visible};

impl Document {
  pub fn ratatui(&self) -> Text<'static> {
    Text::from(self.lines.iter().map(line).collect::<Vec<_>>())
  }
}

pub fn line(value: &Line) -> TerminalLine<'static> {
  TerminalLine::from(
    value
      .spans
      .iter()
      .map(|span| TerminalSpan::styled(visible(&span.content), style(span.style)))
      .collect::<Vec<_>>(),
  )
}

pub fn from_line(value: &TerminalLine<'_>) -> Line {
  Line {
    spans: value
      .spans
      .iter()
      .map(|span| {
        Span::new(
          span.content.to_string(),
          from_style(value.style.patch(span.style)),
        )
      })
      .collect(),
  }
}

pub fn style(value: Style) -> TerminalStyle {
  let mut style = TerminalStyle::default();
  if let Some(color) = value.foreground {
    style = style.fg(terminal_color(color));
  }
  if let Some(color) = value.background {
    style = style.bg(terminal_color(color));
  }
  for (enabled, modifier) in [
    (value.bold, Modifier::BOLD),
    (value.dim, Modifier::DIM),
    (value.italic, Modifier::ITALIC),
    (value.underline, Modifier::UNDERLINED),
    (value.strike, Modifier::CROSSED_OUT),
    (value.reverse, Modifier::REVERSED),
  ] {
    if enabled {
      style = style.add_modifier(modifier);
    }
  }
  style
}

pub fn from_style(value: TerminalStyle) -> Style {
  Style {
    foreground: value.fg.and_then(color),
    background: value.bg.and_then(color),
    bold: value.add_modifier.contains(Modifier::BOLD),
    dim: value.add_modifier.contains(Modifier::DIM),
    italic: value.add_modifier.contains(Modifier::ITALIC),
    underline: value.add_modifier.contains(Modifier::UNDERLINED),
    strike: value.add_modifier.contains(Modifier::CROSSED_OUT),
    reverse: value.add_modifier.contains(Modifier::REVERSED),
  }
}

fn terminal_color(value: Color) -> TerminalColor {
  match value {
    Color::Indexed(index) => TerminalColor::Indexed(index),
    Color::Rgb(r, g, b) => TerminalColor::Rgb(r, g, b),
  }
}

fn color(value: TerminalColor) -> Option<Color> {
  Some(match value {
    TerminalColor::Reset => return None,
    TerminalColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    TerminalColor::Indexed(index) => Color::Indexed(index),
    TerminalColor::Black => Color::Indexed(0),
    TerminalColor::Red => Color::Indexed(1),
    TerminalColor::Green => Color::Indexed(2),
    TerminalColor::Yellow => Color::Indexed(3),
    TerminalColor::Blue => Color::Indexed(4),
    TerminalColor::Magenta => Color::Indexed(5),
    TerminalColor::Cyan => Color::Indexed(6),
    TerminalColor::Gray => Color::Indexed(7),
    TerminalColor::DarkGray => Color::Indexed(8),
    TerminalColor::LightRed => Color::Indexed(9),
    TerminalColor::LightGreen => Color::Indexed(10),
    TerminalColor::LightYellow => Color::Indexed(11),
    TerminalColor::LightBlue => Color::Indexed(12),
    TerminalColor::LightMagenta => Color::Indexed(13),
    TerminalColor::LightCyan => Color::Indexed(14),
    TerminalColor::White => Color::Indexed(15),
  })
}
