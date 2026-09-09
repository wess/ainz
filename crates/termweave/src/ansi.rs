use std::{fmt::Write as _, io};

use crate::{Color, Document, Style, text::visible};

impl Document {
  /// Encode SGR styling. Each row resets its style; no cursor or hyperlink escapes are emitted.
  /// Call `plain()` instead when your application disables color or redirects to a text file.
  pub fn ansi(&self) -> String {
    let mut output = String::new();
    for (index, line) in self.lines.iter().enumerate() {
      if index > 0 {
        output.push('\n');
      }
      for span in &line.spans {
        output.push_str("\x1b[0m");
        style(&mut output, span.style);
        output.push_str(&visible(&span.content));
      }
      output.push_str("\x1b[0m");
    }
    output
  }

  pub fn write_ansi(&self, writer: &mut impl io::Write) -> io::Result<()> {
    writer.write_all(self.ansi().as_bytes())
  }
}

fn style(output: &mut String, style: Style) {
  for (enabled, code) in [
    (style.bold, 1),
    (style.dim, 2),
    (style.italic, 3),
    (style.underline, 4),
    (style.reverse, 7),
    (style.strike, 9),
  ] {
    if enabled {
      let _ = write!(output, "\x1b[{code}m");
    }
  }
  for (color, code) in [(style.foreground, 38), (style.background, 48)] {
    match color {
      Some(Color::Indexed(index)) => {
        let _ = write!(output, "\x1b[{code};5;{index}m");
      }
      Some(Color::Rgb(r, g, b)) => {
        let _ = write!(output, "\x1b[{code};2;{r};{g};{b}m");
      }
      None => {}
    }
  }
}
