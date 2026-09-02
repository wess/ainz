//! The prompt line: the text being written, where the cursor sits in it, and what has already
//! been sent, so up and down walk back through earlier prompts the way every other harness does.

#[derive(Default)]
pub(super) struct Input {
  text: String,
  // a byte offset, always on a character boundary
  cursor: usize,
  history: Vec<String>,
  // where in history the walk currently is, and the draft it started from
  recalled: Option<usize>,
  draft: String,
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

  /// Takes the line to send, and remembers it for the next walk back.
  pub fn submit(&mut self) -> String {
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

#[cfg(test)]
mod tests {
  use super::Input;

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
}
