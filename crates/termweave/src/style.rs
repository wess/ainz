/// A terminal palette entry or true-color value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
  Indexed(u8),
  Rgb(u8, u8, u8),
}

/// Absolute styling for one span. Missing colors use the terminal's defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
  pub foreground: Option<Color>,
  pub background: Option<Color>,
  pub bold: bool,
  pub dim: bool,
  pub italic: bool,
  pub underline: bool,
  pub strike: bool,
  pub reverse: bool,
}

impl Style {
  pub(crate) fn overlay(self, other: Self) -> Self {
    Self {
      foreground: other.foreground.or(self.foreground),
      background: other.background.or(self.background),
      bold: self.bold || other.bold,
      dim: self.dim || other.dim,
      italic: self.italic || other.italic,
      underline: self.underline || other.underline,
      strike: self.strike || other.strike,
      reverse: self.reverse || other.reverse,
    }
  }
}

/// Semantic styles, independent of a particular terminal backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
  pub text: Style,
  pub heading: Style,
  pub code: Style,
  pub link: Style,
  pub marker: Style,
}

impl Default for Theme {
  fn default() -> Self {
    Self {
      text: Style::default(),
      heading: Style {
        bold: true,
        ..Style::default()
      },
      code: Style {
        foreground: Some(Color::Indexed(6)),
        ..Style::default()
      },
      link: Style {
        foreground: Some(Color::Indexed(6)),
        underline: true,
        ..Style::default()
      },
      marker: Style {
        dim: true,
        ..Style::default()
      },
    }
  }
}
