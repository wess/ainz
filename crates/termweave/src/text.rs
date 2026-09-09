use unicode_width::UnicodeWidthStr;

use crate::Style;

/// Owned text with one style. Control characters are escaped when rendered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
  pub content: String,
  pub style: Style,
}

impl Span {
  pub fn new(content: impl Into<String>, style: Style) -> Self {
    Self {
      content: content.into(),
      style,
    }
  }
}

/// One physical line of terminal text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
  pub spans: Vec<Span>,
}

impl Line {
  pub fn plain(text: impl Into<String>) -> Self {
    Self::styled(text, Style::default())
  }

  pub fn styled(text: impl Into<String>, style: Style) -> Self {
    Self {
      spans: vec![Span::new(text, style)],
    }
  }

  pub fn text(&self) -> String {
    self
      .spans
      .iter()
      .map(|span| visible(&span.content))
      .collect()
  }

  pub fn width(&self) -> usize {
    UnicodeWidthStr::width(self.text().as_str())
  }

  pub(crate) fn push(&mut self, text: impl AsRef<str>, style: Style) {
    let text = visible(text.as_ref());
    if text.is_empty() {
      return;
    }
    if let Some(span) = self.spans.last_mut()
      && span.style == style
    {
      span.content.push_str(&text);
    } else {
      self.spans.push(Span::new(text, style));
    }
  }

  pub(crate) fn append(&mut self, other: &Self) {
    for span in &other.spans {
      self.push(&span.content, span.style);
    }
  }
}

/// A replaceable rendering snapshot; rows contain no terminal control sequences.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
  pub lines: Vec<Line>,
}

impl Document {
  pub fn plain(&self) -> String {
    self
      .lines
      .iter()
      .map(Line::text)
      .collect::<Vec<_>>()
      .join("\n")
  }
}

// terminal control sequences in a document must stay text, including in raw HTML and URLs
pub(crate) fn visible(text: &str) -> String {
  let mut output = String::with_capacity(text.len());
  for ch in text.chars() {
    match ch {
      '\t' => output.push_str("    "),
      '\u{00}'..='\u{1f}' => output.push(char::from_u32(0x2400 + ch as u32).unwrap_or('�')),
      '\u{7f}' => output.push('␡'),
      '\u{80}'..='\u{9f}' => output.push('�'),
      _ => output.push(ch),
    }
  }
  output
}
