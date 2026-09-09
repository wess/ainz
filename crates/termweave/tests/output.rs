use termweave::{Color, Document, Line, Options, Style, render};

#[test]
fn ansi_encodes_styles_and_resets_each_line() {
  let style = Style {
    foreground: Some(Color::Rgb(1, 2, 3)),
    background: Some(Color::Indexed(4)),
    bold: true,
    italic: true,
    underline: true,
    ..Style::default()
  };
  let document = Document {
    lines: vec![Line::styled("styled", style), Line::plain("plain")],
  };
  assert_eq!(
    document.ansi(),
    "\x1b[0m\x1b[1m\x1b[3m\x1b[4m\x1b[38;2;1;2;3m\x1b[48;5;4mstyled\x1b[0m\n\x1b[0mplain\x1b[0m"
  );
  assert_eq!(document.plain(), "styled\nplain");
  let mut output = Vec::new();
  document.write_ansi(&mut output).unwrap();
  assert_eq!(output, document.ansi().as_bytes());
}

#[test]
fn ansi_never_passes_through_input_control_sequences() {
  let source = "\x1b[2Jtext\x07\x1b]52;c;secret\x1b\\\u{9b}31m";
  let document = Document {
    lines: vec![Line::plain(source)],
  };
  let ansi = document.ansi();
  assert!(!ansi.contains("\x1b[2J"));
  assert!(!ansi.contains("\x1b]"));
  assert!(!ansi.contains('\u{9b}'));
  assert!(!ansi.contains('\x07'));
}

#[test]
fn output_reports_writer_failures() {
  struct Broken;
  impl std::io::Write for Broken {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
      Err(std::io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
      Ok(())
    }
  }
  assert_eq!(
    render("hello", &Options::default())
      .write_ansi(&mut Broken)
      .unwrap_err()
      .kind(),
    std::io::ErrorKind::BrokenPipe
  );
}
