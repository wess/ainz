//! Styled text for panels and bars, written the way irssi and BitchX themes write it: `%R` for
//! bright red, `%_` for bold, `%n` to reset, and `%{accent}` for a colour from the theme. Lines
//! are parsed once when a script sets them, so drawing them costs nothing extra.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Color {
  /// One of the sixteen terminal colours: 0–7 normal, 8–15 bright.
  Ansi(u8),
  Rgb(u8, u8, u8),
  /// A colour the theme names, such as `accent` or `error`, so it follows `/theme`.
  Role(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
  pub fg: Option<Color>,
  pub bg: Option<Color>,
  pub bold: bool,
  pub italic: bool,
  pub underline: bool,
  pub reverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
  pub text: String,
  pub style: Style,
}

pub type Line = Vec<Segment>;

/// The letters irssi uses, in terminal order: black, red, green, yellow, blue, magenta, cyan,
/// white. Lowercase is the normal colour, uppercase the bright one.
const LETTERS: &[u8; 8] = b"krgybmcw";

/// A line from its format codes. Anything that is not a known code is kept as written, so a
/// stray `%` in a file name survives.
pub fn parse(text: &str) -> Line {
  let mut line = Line::new();
  let mut style = Style::default();
  let mut run = String::new();
  let mut chars = text.chars().peekable();
  let flush = |run: &mut String, style: &Style, line: &mut Line| {
    if !run.is_empty() {
      line.push(Segment {
        text: std::mem::take(run),
        style: style.clone(),
      });
    }
  };
  while let Some(ch) = chars.next() {
    if ch != '%' {
      run.push(ch);
      continue;
    }
    let Some(&code) = chars.peek() else {
      run.push('%');
      break;
    };
    let mut next = style.clone();
    match code {
      '%' => {
        chars.next();
        run.push('%');
        continue;
      }
      'n' | 'N' => next = Style::default(),
      '_' => next.bold = !next.bold,
      'U' => next.underline = !next.underline,
      'I' => next.italic = !next.italic,
      '8' => next.reverse = !next.reverse,
      '0'..='7' => next.bg = Some(Color::Ansi(code as u8 - b'0')),
      '{' => {
        let mut name = String::new();
        let mut ahead = chars.clone();
        ahead.next();
        let mut closed = false;
        for ch in ahead.by_ref() {
          if ch == '}' {
            closed = true;
            break;
          }
          name.push(ch);
        }
        match closed.then(|| braced(&name, &style)).flatten() {
          Some(braced) => {
            next = braced;
            chars = ahead;
            flush(&mut run, &style, &mut line);
            style = next;
            continue;
          }
          None => {
            run.push('%');
            continue;
          }
        }
      }
      letter => match LETTERS
        .iter()
        .position(|known| *known == letter.to_ascii_lowercase() as u8)
      {
        Some(index) => {
          let bright = if letter.is_ascii_uppercase() { 8 } else { 0 };
          next.fg = Some(Color::Ansi(index as u8 + bright));
        }
        None => {
          run.push('%');
          continue;
        }
      },
    }
    chars.next();
    flush(&mut run, &style, &mut line);
    style = next;
  }
  flush(&mut run, &style, &mut line);
  line
}

/// `%{accent}`, `%{#ff8800}`, `%{bg:muted}` or `%{bg:#202020}`.
fn braced(name: &str, style: &Style) -> Option<Style> {
  let mut next = style.clone();
  let (background, name) = match name.strip_prefix("bg:") {
    Some(rest) => (true, rest),
    None => (false, name),
  };
  let color = color(name)?;
  if background {
    next.bg = Some(color);
  } else {
    next.fg = Some(color);
  }
  Some(next)
}

/// A colour by name: `#rrggbb`, one of the sixteen terminal names (`red`, `brightred`), or a
/// theme role.
pub fn color(name: &str) -> Option<Color> {
  let name = name.trim();
  if let Some(hex) = name.strip_prefix('#') {
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
      return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    return Some(Color::Rgb(
      (value >> 16) as u8,
      (value >> 8) as u8,
      value as u8,
    ));
  }
  const NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  ];
  let lower = name.to_ascii_lowercase();
  let (bright, base) = match lower.strip_prefix("bright") {
    Some(base) => (8, base),
    None => (0, lower.as_str()),
  };
  if let Some(index) = NAMES.iter().position(|known| *known == base) {
    return Some(Color::Ansi(index as u8 + bright));
  }
  let role = !lower.is_empty()
    && lower
      .bytes()
      .all(|byte| byte.is_ascii_lowercase() || byte == b'_');
  role.then_some(Color::Role(lower))
}

/// The text of a line without its styling, for measuring and for tests.
pub fn plain(line: &Line) -> String {
  line.iter().map(|segment| segment.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn colours_and_resets_split_a_line_into_styled_runs() {
    let line = parse("%Rred%n plain %gok");
    assert_eq!(plain(&line), "red plain ok");
    assert_eq!(line[0].style.fg, Some(Color::Ansi(9)));
    assert_eq!(line[1].style, Style::default());
    assert_eq!(line[2].style.fg, Some(Color::Ansi(2)));
  }

  #[test]
  fn bold_toggles_and_backgrounds_stack_on_the_colour() {
    let line = parse("%_%4%Wbar%_ thin");
    assert!(line[0].style.bold);
    assert_eq!(line[0].style.bg, Some(Color::Ansi(4)));
    assert_eq!(line[0].style.fg, Some(Color::Ansi(15)));
    assert!(!line[1].style.bold);
    assert_eq!(line[1].style.bg, Some(Color::Ansi(4)));
  }

  #[test]
  fn braces_name_a_theme_role_or_a_hex_colour() {
    let line = parse("%{accent}a%{#ff8800}b%{bg:muted}c");
    assert_eq!(line[0].style.fg, Some(Color::Role("accent".into())));
    assert_eq!(line[1].style.fg, Some(Color::Rgb(255, 136, 0)));
    assert_eq!(line[2].style.bg, Some(Color::Role("muted".into())));
  }

  #[test]
  fn anything_that_is_not_a_code_is_kept_as_written() {
    assert_eq!(plain(&parse("100%% done")), "100% done");
    assert_eq!(plain(&parse("50% off %q")), "50% off %q");
    assert_eq!(plain(&parse("%{unclosed")), "%{unclosed");
    assert_eq!(plain(&parse("trailing %")), "trailing %");
  }

  #[test]
  fn colours_are_found_by_name() {
    assert_eq!(color("brightred"), Some(Color::Ansi(9)));
    assert_eq!(color("blue"), Some(Color::Ansi(4)));
    assert_eq!(color("#00ff00"), Some(Color::Rgb(0, 255, 0)));
    assert_eq!(color("bar_text"), Some(Color::Role("bar_text".into())));
    assert_eq!(color("#nothex"), None);
  }
}
