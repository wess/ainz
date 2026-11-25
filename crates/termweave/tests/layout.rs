use termweave::{Color, Line, Options, Span, Style, render, wrap};

#[test]
fn hanging_prefixes_preserve_styles_and_unicode() {
  let source =
    "**界界界界界界界界** and e\u{301} 👩🏽‍💻 👨‍👩‍👧‍👦 words that wrap.\n\n- nested words that also wrap.";
  for width in [0, 1, 2, 8, 18, 40, 80] {
    let options = Options {
      width,
      prefix: Line::styled(
        "<nick> ",
        Style {
          foreground: Some(Color::Indexed(2)),
          ..Style::default()
        },
      ),
      ..Options::default()
    };
    let document = render(source, &options);
    assert!(
      document.lines.iter().all(|line| line.width() <= width),
      "width {width}: {document:?}"
    );
    if width == 0 {
      assert!(document.lines.is_empty());
    }
    if width >= 18 {
      assert!(document.lines[0].text().starts_with("<nick> "));
      assert!(
        document.lines[1..]
          .iter()
          .all(|line| line.text().starts_with("       "))
      );
      let text = document.plain();
      assert_eq!(text.matches('界').count(), 8);
      assert!(text.contains("e\u{301}"));
      assert!(text.contains("👩🏽‍💻"));
      assert!(text.contains("👨‍👩‍👧‍👦"));
    }
  }
}

#[test]
fn plain_wrapping_does_not_parse_markdown_or_drop_long_words() {
  let text = format!("**literal** {} END", "x".repeat(200));
  let document = wrap(
    vec![Line::plain(&text)],
    &Options {
      width: 17,
      ..Options::default()
    },
  );
  let recovered: String = document.lines.iter().map(Line::text).collect();
  assert_eq!(recovered, text);
  assert!(document.lines.iter().all(|line| line.width() <= 17));
}

#[test]
fn tabs_and_controls_are_measured_as_they_are_drawn() {
  let line = Line {
    spans: vec![Span::new("a\tb\x1b[31m", Style::default())],
  };
  let document = wrap(
    vec![line],
    &Options {
      width: 8,
      ..Options::default()
    },
  );
  assert!(document.lines.iter().all(|line| line.width() <= 8));
  let text: String = document.lines.iter().map(Line::text).collect();
  assert_eq!(text, "a    b␛[31m");
}

#[test]
fn fenced_code_preserves_spaces_and_wraps_without_eating_indentation() {
  let source = "```\n    a  b\n\tlonglonglonglonglonglong\n```";
  let document = render(
    source,
    &Options {
      width: 14,
      ..Options::default()
    },
  );
  let rows: Vec<_> = document.lines.iter().map(Line::text).collect();
  assert_eq!(rows[0], "    a  b");
  assert_eq!(rows[1..].concat(), "    longlonglonglonglonglong");
  assert!(document.lines.iter().all(|line| line.width() <= 14));
}

#[test]
fn a_grapheme_crossing_style_boundaries_is_not_split() {
  let red = Style {
    foreground: Some(Color::Indexed(1)),
    ..Style::default()
  };
  let green = Style {
    foreground: Some(Color::Indexed(2)),
    ..Style::default()
  };
  let line = Line {
    spans: vec![
      Span::new("👩", red),
      Span::new("\u{200d}💻e", green),
      Span::new("\u{301}", red),
    ],
  };
  let document = wrap(
    vec![line],
    &Options {
      width: 2,
      ..Options::default()
    },
  );
  assert_eq!(document.lines[0].text(), "👩‍💻");
  assert_eq!(document.lines[1].text(), "e\u{301}");
  assert_eq!(document.lines[0].spans[0].style, red);
  assert_eq!(document.lines[1].spans[0].style, green);
}

#[test]
fn copied_commands_have_no_decoration_on_any_wrapped_row() {
  let command = "node .gitlab/ci/audit-lockfile.cjs --level critical | tee audit.log";
  for width in [8, 24, 80] {
    let options = Options {
      width,
      ..Options::default()
    };
    let document = render(&format!("```bash\n{command}\n```"), &options);
    let copied: String = document.lines.iter().map(Line::text).collect();
    assert_eq!(copied, command);
    assert!(document.lines.iter().all(|line| line.width() <= width));
    assert!(
      document
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .all(|span| span.style.foreground == options.theme.code.foreground)
    );
  }
}

#[test]
fn code_keeps_blank_lines_and_indented_blocks_without_fence_labels() {
  let options = Options::default();
  assert_eq!(
    render("```bash\nfirst\n\n  second\n```", &options).plain(),
    "first\n\n  second"
  );
  assert_eq!(
    render("    first\n    second\n", &options).plain(),
    "first\nsecond"
  );
  assert_eq!(
    render("before\n\n```bash\ncommand\n```\n\nafter", &options).plain(),
    "before\n\ncommand\n\nafter"
  );
}
