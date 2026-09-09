//! Markdown rendered as owned terminal lines, with ANSI and optional Ratatui adapters.
//!
//! ```
//! use termweave::{Options, Stream, render};
//! let options = Options { width: 40, ..Options::default() };
//! assert_eq!(render("**hello**", &options).plain(), "hello");
//! let mut stream = Stream::default();
//! stream.push("**hel");
//! let pending = stream.snapshot(&options);
//! stream.push("lo**");
//! assert_eq!(stream.finish(&options).plain(), "hello");
//! ```

mod ansi;
mod inline;
mod layout;
mod parse;
mod style;
mod table;
mod text;

#[cfg(feature = "ratatui")]
pub mod ratatui;

pub use style::{Color, Style, Theme};
pub use text::{Document, Line, Span};

/// Layout settings. `width` includes the prefix and all Markdown indentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
  pub width: usize,
  pub theme: Theme,
  /// Appears on the first row; subsequent rows use equally wide spaces.
  pub prefix: Line,
}

impl Default for Options {
  fn default() -> Self {
    Self {
      width: 80,
      theme: Theme::default(),
      prefix: Line::default(),
    }
  }
}

/// Render a complete Markdown document. CommonMark plus tables, tasks, and strikethrough.
pub fn render(source: &str, options: &Options) -> Document {
  layout::document(parse::markdown(source, options, true), options)
}

/// Wrap already styled lines without interpreting them as Markdown.
pub fn wrap(lines: Vec<Line>, options: &Options) -> Document {
  layout::document(
    lines.into_iter().map(layout::LogicalLine::new).collect(),
    options,
  )
}

/// Accumulates UTF-8 chunks and renders replaceable snapshots of all text received so far.
///
/// Snapshots reparse the source: later delimiters or reference definitions can change earlier
/// rows. Replace the previous snapshot rather than appending its lines to scrollback.
#[derive(Clone, Debug, Default)]
pub struct Stream {
  source: String,
}

impl Stream {
  pub fn push(&mut self, chunk: &str) {
    self.source.push_str(chunk);
  }

  pub fn source(&self) -> &str {
    &self.source
  }

  pub fn snapshot(&self, options: &Options) -> Document {
    layout::document(parse::markdown(&self.source, options, false), options)
  }

  pub fn finish(self, options: &Options) -> Document {
    render(&self.source, options)
  }
}
