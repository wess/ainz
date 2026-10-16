//! The prompt line: the text being written, where the cursor sits in it, and what has already
//! been sent, so up and down walk back through earlier prompts the way every other harness does.

/// Where keys go: straight into the line, or to vim's normal-mode verbs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
  #[default]
  Insert,
  Normal,
}

#[derive(Default)]
pub(super) struct Input {
  text: String,
  // a byte offset, always on a character boundary
  cursor: usize,
  history: Vec<String>,
  // where in history the walk currently is, and the draft it started from
  recalled: Option<usize>,
  draft: String,
  mode: Mode,
}

impl Input {
  pub fn with_history(history: Vec<String>) -> Self {
    Self {
      history,
      ..Self::default()
    }
  }

  pub fn as_str(&self) -> &str {
    &self.text
  }

  pub fn cursor(&self) -> usize {
    self.cursor
  }

  pub fn mode(&self) -> Mode {
    self.mode
  }

  pub fn set_mode(&mut self, mode: Mode) {
    self.mode = mode;
    // normal mode sits on a character, so it cannot sit past the end of the line
    if mode == Mode::Normal && self.cursor >= self.text.len() {
      self.cursor = self.previous_char();
    }
  }

  /// vim's `a`: one character right, staying on the line.
  pub fn append(&mut self) {
    self.cursor = self.next_char();
    self.mode = Mode::Insert;
  }

  /// Replaces the line outright, as command completion does.
  pub fn set(&mut self, text: String) {
    self.text = text;
    self.cursor = self.text.len();
    self.recalled = None;
  }

  pub fn clear(&mut self) {
    self.set(String::new());
  }

  pub fn insert(&mut self, ch: char) {
    self.text.insert(self.cursor, ch);
    self.cursor += ch.len_utf8();
  }

  pub fn insert_str(&mut self, text: &str) {
    self.text.insert_str(self.cursor, text);
    self.cursor += text.len();
  }

  pub fn backspace(&mut self) {
    let start = self.previous_char();
    self.text.replace_range(start..self.cursor, "");
    self.cursor = start;
  }

  pub fn delete(&mut self) {
    let end = self.next_char();
    self.text.replace_range(self.cursor..end, "");
  }

  pub fn left(&mut self) {
    self.cursor = self.previous_char();
  }

  pub fn right(&mut self) {
    self.cursor = self.next_char();
  }

  pub fn word_left(&mut self) {
    self.cursor = self.word_start();
  }

  pub fn word_right(&mut self) {
    self.cursor = self.word_end();
  }

  /// Puts the cursor where the pointer was, counting rows and characters as drawn.
  /// Moves the caret to a row of the text as drawn, wrapped to `width`, and a column in it.
  pub fn place(&mut self, row: usize, column: usize, width: usize) {
    let rows = wrap(&self.text, width);
    let Some(range) = rows.get(row).cloned() else {
      self.cursor = self.text.len();
      return;
    };
    let mut used = 0;
    self.cursor = self.text[range.clone()]
      .char_indices()
      .find_map(|(at, character)| {
        used += columns(character);
        (used > column).then_some(range.start + at)
      })
      .unwrap_or(range.end);
  }

  /// The row and column the caret is drawn at once the text is wrapped to `width`.
  pub fn caret(&self, width: usize) -> (usize, usize) {
    let before = &self.text[..self.cursor];
    let rows = wrap(before, width);
    let last = rows[rows.len() - 1].clone();
    (rows.len() - 1, before[last].chars().map(columns).sum())
  }

  pub fn home(&mut self) {
    self.cursor = 0;
  }

  pub fn end(&mut self) {
    self.cursor = self.text.len();
  }

  pub fn delete_word(&mut self) {
    let start = self.word_start();
    self.text.replace_range(start..self.cursor, "");
    self.cursor = start;
  }

  pub fn kill_to_start(&mut self) {
    self.text.replace_range(..self.cursor, "");
    self.cursor = 0;
  }

  pub fn kill_to_end(&mut self) {
    self.text.truncate(self.cursor);
  }

  pub fn delete_word_forward(&mut self) {
    let end = self.word_end();
    self.text.replace_range(self.cursor..end, "");
  }

  /// Takes the line to send, and remembers it for the next walk back.
  pub fn submit(&mut self) -> String {
    self.mode = Mode::Insert;
    let text = std::mem::take(&mut self.text);
    self.cursor = 0;
    self.recalled = None;
    self.draft.clear();
    if !text.trim().is_empty() && self.history.last() != Some(&text) {
      self.history.push(text.clone());
    }
    text
  }

  /// Walks to an older prompt. The line being written is kept, so walking back down returns it.
  pub fn previous(&mut self) -> bool {
    let index = match self.recalled {
      Some(0) => return false,
      Some(index) => index - 1,
      None => {
        self.draft = self.text.clone();
        self.history.len().saturating_sub(1)
      }
    };
    let Some(entry) = self.history.get(index).cloned() else {
      return false;
    };
    self.recalled = Some(index);
    self.text = entry;
    self.cursor = self.text.len();
    true
  }

  pub fn next(&mut self) -> bool {
    let Some(index) = self.recalled else {
      return false;
    };
    match self.history.get(index + 1) {
      Some(entry) => {
        self.recalled = Some(index + 1);
        self.text = entry.clone();
      }
      None => {
        self.recalled = None;
        self.text = std::mem::take(&mut self.draft);
      }
    }
    self.cursor = self.text.len();
    true
  }

  fn previous_char(&self) -> usize {
    self.text[..self.cursor]
      .chars()
      .next_back()
      .map_or(0, |ch| self.cursor - ch.len_utf8())
  }

  fn next_char(&self) -> usize {
    self.text[self.cursor..]
      .chars()
      .next()
      .map_or(self.cursor, |ch| self.cursor + ch.len_utf8())
  }

  // a word boundary is whitespace, so ctrl+w eats a path or a flag whole
  fn word_start(&self) -> usize {
    let head = &self.text[..self.cursor];
    let trimmed = head.trim_end();
    match trimmed.rfind(char::is_whitespace) {
      Some(index) => index + head[index..].chars().next().map_or(1, char::len_utf8),
      None => 0,
    }
  }

  fn word_end(&self) -> usize {
    let tail = &self.text[self.cursor..];
    let skipped = tail.len() - tail.trim_start().len();
    match tail.trim_start().find(char::is_whitespace) {
      Some(index) => self.cursor + skipped + index,
      None => self.text.len(),
    }
  }
}

/// The rows the text takes once wrapped to `width` columns, as byte ranges. A newline ends a
/// row, so does a character that would not fit, and so does a row that is exactly full, which
/// is why the caret after one sits at the start of the next. Columns are display columns, so a
/// wide character takes two and the caret lands where the terminal draws it.
pub(super) fn wrap(text: &str, width: usize) -> Vec<std::ops::Range<usize>> {
  let width = width.max(1);
  let mut rows = Vec::new();
  let (mut start, mut column) = (0, 0);
  for (index, character) in text.char_indices() {
    if character == '\n' {
      rows.push(start..index);
      (start, column) = (index + 1, 0);
      continue;
    }
    let wide = columns(character);
    if column > 0 && column + wide > width {
      rows.push(start..index);
      (start, column) = (index, 0);
    }
    column += wide;
    if column >= width {
      let end = index + character.len_utf8();
      rows.push(start..end);
      (start, column) = (end, 0);
    }
  }
  rows.push(start..text.len());
  rows
}

// a control character is dropped when drawn, so it takes no room
fn columns(character: char) -> usize {
  match character.is_control() {
    true => 0,
    false => unicode_width::UnicodeWidthChar::width(character).unwrap_or(0),
  }
}

#[cfg(test)]
mod tests {
  use super::{Input, Mode};

  fn typed(text: &str) -> Input {
    let mut input = Input::default();
    input.insert_str(text);
    input
  }

  #[test]
  fn up_walks_back_through_what_was_sent() {
    let mut input = Input::default();
    input.insert_str("first");
    input.submit();
    input.insert_str("second");
    input.submit();
    input.insert_str("draft");

    assert!(input.previous());
    assert_eq!(input.as_str(), "second");
    assert!(input.previous());
    assert_eq!(input.as_str(), "first");
    // nothing older to reach
    assert!(!input.previous());

    input.next();
    assert_eq!(input.as_str(), "second");
    input.next();
    // back at the line that was being written when the walk started
    assert_eq!(input.as_str(), "draft");
  }

  #[test]
  fn editing_happens_at_the_cursor() {
    let mut input = typed("hello world");
    input.word_left();
    input.insert_str("wide ");
    assert_eq!(input.as_str(), "hello wide world");

    input.home();
    input.right();
    input.delete();
    assert_eq!(input.as_str(), "hllo wide world");

    input.end();
    input.delete_word();
    assert_eq!(input.as_str(), "hllo wide ");
  }

  #[test]
  fn a_multibyte_line_keeps_its_character_boundaries() {
    let mut input = typed("café ☕");
    input.backspace();
    assert_eq!(input.as_str(), "café ");
    input.left();
    input.backspace();
    assert_eq!(input.as_str(), "caf ");
    assert_eq!(input.cursor(), 3);
  }

  #[test]
  fn normal_mode_sits_on_the_last_character_rather_than_past_it() {
    let mut input = typed("go");

    input.set_mode(Mode::Normal);
    assert_eq!(input.cursor(), 1);

    input.append();
    assert_eq!(input.mode(), Mode::Insert);
    assert_eq!(input.cursor(), 2);
  }

  #[test]
  fn the_same_prompt_twice_is_remembered_once() {
    let mut input = typed("ls");
    input.submit();
    let mut input = Input::with_history(vec!["ls".into()]);
    input.insert_str("ls");
    input.submit();

    input.previous();
    assert_eq!(input.as_str(), "ls");
    assert!(!input.previous());
  }

  #[test]
  fn a_long_line_wraps_and_the_caret_follows_it() {
    let input = typed("abcdefghij");
    assert_eq!(super::wrap(input.as_str(), 4), [0..4, 4..8, 8..10]);
    assert_eq!(input.caret(4), (2, 2));
  }

  #[test]
  fn a_full_row_puts_the_caret_at_the_start_of_the_next() {
    let input = typed("abcd");
    assert_eq!(input.caret(4), (1, 0));
  }

  #[test]
  fn a_wide_character_takes_two_columns() {
    let input = typed("日本語");
    assert_eq!(super::wrap(input.as_str(), 4).len(), 2);
    assert_eq!(input.caret(10), (0, 6));
  }

  #[test]
  fn a_click_lands_on_the_character_drawn_there_in_a_wrapped_row() {
    let mut input = typed("abcdefghij");
    input.place(1, 2, 4);
    assert_eq!(input.cursor(), 6);
    input.place(9, 0, 4);
    assert_eq!(input.cursor(), 10);
  }
}
