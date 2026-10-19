#[derive(Default)]
pub(crate) struct Input {
  pub escape: Vec<u8>,
  text: Vec<u8>,
  paste: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Key {
  Up,
  Down,
  Enter,
  Backspace,
  Clear,
  Cancel,
  Char(char),
  Text(char),
}

pub(crate) fn parse(input: &mut Input, byte: u8) -> Option<Key> {
  if byte == 27 {
    input.escape.clear();
    input.escape.push(byte);
    return None;
  }
  if !input.escape.is_empty() {
    input.escape.push(byte);
    if input.escape.len() == 2 {
      if !matches!(byte, b'[' | b'O') {
        input.escape.clear();
      }
      return None;
    }
    if (0x40..=0x7e).contains(&byte) || input.escape.len() > 32 {
      let key = match input.escape.as_slice() {
        b"\x1b[A" | b"\x1bOA" if !input.paste => Some(Key::Up),
        b"\x1b[B" | b"\x1bOB" if !input.paste => Some(Key::Down),
        b"\x1b[200~" => {
          input.paste = true;
          None
        }
        b"\x1b[201~" => {
          input.paste = false;
          None
        }
        _ => None,
      };
      input.escape.clear();
      return key;
    }
    return None;
  }
  match byte {
    b'\r' | b'\n' if !input.paste => Some(Key::Enter),
    8 | 127 if !input.paste => Some(Key::Backspace),
    21 if !input.paste => Some(Key::Clear),
    3 if !input.paste => Some(Key::Cancel),
    32..=126 | 128..=255 => {
      input.text.push(byte);
      match std::str::from_utf8(&input.text) {
        Ok(text) => {
          let character = text.chars().next();
          input.text.clear();
          character.map(|c| {
            if input.paste {
              Key::Text(c)
            } else {
              Key::Char(c)
            }
          })
        }
        Err(error) if error.error_len().is_some() => {
          input.text.clear();
          None
        }
        _ => None,
      }
    }
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn split_unicode_and_terminal_sequences_keep_pasted_shortcuts_as_search_text() {
    let mut input = Input::default();
    let bytes = "\x1b[A\x1b[B\x1b[200~new\r\x1b[201~/café\x15".as_bytes();
    let keys: Vec<_> = bytes
      .iter()
      .filter_map(|byte| parse(&mut input, *byte))
      .collect();
    assert_eq!(
      keys,
      vec![
        Key::Up,
        Key::Down,
        Key::Text('n'),
        Key::Text('e'),
        Key::Text('w'),
        Key::Char('/'),
        Key::Char('c'),
        Key::Char('a'),
        Key::Char('f'),
        Key::Char('é'),
        Key::Clear
      ]
    );
  }
}
