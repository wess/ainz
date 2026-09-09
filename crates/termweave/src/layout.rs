use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{Document, Line, Options, Style, text::visible};

#[derive(Clone, Debug, Default)]
pub(crate) struct LogicalLine {
  pub content: Line,
  pub prefix: Line,
  pub continuation: Line,
  pub literal: bool,
}

impl LogicalLine {
  pub fn new(content: Line) -> Self {
    Self {
      content,
      ..Self::default()
    }
  }

  pub fn indent(&mut self, first: &Line, rest: &Line) {
    let mut prefix = first.clone();
    prefix.append(&self.prefix);
    self.prefix = prefix;
    let mut continuation = rest.clone();
    continuation.append(&self.continuation);
    self.continuation = continuation;
  }
}

pub(crate) fn document(lines: Vec<LogicalLine>, options: &Options) -> Document {
  if options.width == 0 {
    return Document::default();
  }
  let prefix = clipped(&options.prefix, options.width.saturating_sub(2));
  let blank = Line::plain(" ".repeat(prefix.width()));
  let mut output = Vec::new();
  for line in lines {
    let mut first = if output.is_empty() {
      prefix.clone()
    } else {
      blank.clone()
    };
    first.append(&line.prefix);
    let mut rest = blank.clone();
    rest.append(&line.continuation);
    output.extend(wrapped(
      &line.content,
      &first,
      &rest,
      options.width,
      line.literal,
    ));
  }
  if output.is_empty() && !prefix.spans.is_empty() {
    output.push(prefix);
  }
  Document { lines: output }
}

fn graphemes(line: &Line) -> Vec<(String, Style, usize)> {
  let mut text = String::new();
  let mut styles = Vec::new();
  for span in &line.spans {
    let content = visible(&span.content);
    if content.is_empty() {
      continue;
    }
    text.push_str(&content);
    styles.push((text.len(), span.style));
  }
  let mut index = 0;
  text
    .grapheme_indices(true)
    .map(|(offset, text)| {
      while offset >= styles[index].0 {
        index += 1;
      }
      (
        text.to_string(),
        styles[index].1,
        UnicodeWidthStr::width(text),
      )
    })
    .collect()
}

pub(crate) fn clipped(line: &Line, width: usize) -> Line {
  let mut output = Line::default();
  let mut used = 0;
  for (text, style, cells) in graphemes(line) {
    if used + cells > width {
      break;
    }
    output.push(text, style);
    used += cells;
  }
  output
}

pub(crate) fn wrapped(
  line: &Line,
  first: &Line,
  rest: &Line,
  width: usize,
  literal: bool,
) -> Vec<Line> {
  if width == 0 {
    return Vec::new();
  }
  let cells = graphemes(line);
  let mut start = 0;
  let mut lines = Vec::new();
  loop {
    let mut row = clipped(
      if lines.is_empty() { first } else { rest },
      width.saturating_sub(2),
    );
    let available = width - row.width();
    let mut end = start;
    let mut used = 0;
    while end < cells.len() && used + cells[end].2 <= available {
      used += cells[end].2;
      end += 1;
    }
    if end < cells.len()
      && !literal
      && let Some(space) = (start..end)
        .rev()
        .find(|&i| cells[i].0 == " " && cells[start..i].iter().any(|cell| cell.0 != " "))
    {
      end = space + 1;
    }
    if end == start && start < cells.len() {
      // a two-cell glyph cannot fit a one-cell terminal; do not split its grapheme
      row.push("�", cells[start].1);
      end += 1;
    } else {
      for (text, style, _) in &cells[start..end] {
        row.push(text, *style);
      }
    }
    lines.push(row);
    if end == cells.len() {
      break;
    }
    start = end;
  }
  lines
}
