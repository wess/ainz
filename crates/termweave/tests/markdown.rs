use termweave::{Options, render};

const SAMPLE: &str = "# Heading\n\nA **bold _nested_** word, ~~removed~~ and `code`.\n\n> quote\n>\n> - [x] checked\n> - [ ] pending\n\n3. third\n4. fourth\n\n[guide][ref] and ![diagram](image.png)\n\n[ref]: https://example.com/docs\n\n```rust\n  let literal = \"**keep**\";\n```\n\n---\n\nfootnote[^n]\n\n[^n]: detail";

#[test]
fn commonmark_blocks_and_nested_styles_are_rendered() {
  let document = render(SAMPLE, &Options::default());
  let plain = document.plain();
  for expected in [
    "Heading",
    "A bold nested word, removed and code.",
    "│ quote",
    "- [x] checked",
    "- [ ] pending",
    "3. third",
    "4. fourth",
    "guide (https://example.com/docs)",
    "[image: diagram] (image.png)",
    "  let literal = \"**keep**\";",
    "footnote[n]",
    "[n] detail",
  ] {
    assert!(plain.contains(expected), "missing {expected}:\n{plain}");
  }
  let spans: Vec<_> = document.lines.iter().flat_map(|line| &line.spans).collect();
  assert!(
    spans
      .iter()
      .any(|span| span.content.contains("nested") && span.style.bold && span.style.italic)
  );
  assert!(
    spans
      .iter()
      .any(|span| span.content.contains("removed") && span.style.strike)
  );
  assert!(
    spans
      .iter()
      .any(|span| span.content == "guide" && span.style.underline)
  );
  assert!(
    spans
      .iter()
      .any(|span| span.content.contains("Heading") && span.style.bold)
  );
}

#[test]
fn escaping_entities_setext_and_code_follow_commonmark() {
  let source = "Title\n=====\n\n\\*literal\\* &amp; __strong__ and ``a ` b``.\n\nline one\nline two  \nline three";
  let document = render(source, &Options::default());
  assert_eq!(
    document.plain(),
    "Title\n\n*literal* & strong and a ` b.\n\nline one line two\nline three"
  );
}

#[test]
fn tables_align_and_keep_all_cells_on_narrow_screens() {
  let source = "| Name | Count | Note |\n|:--|--:|:--:|\n| apple | 12 | hello world |\n| berry | 7 | **ready** |";
  for width in [12, 24, 40, 80] {
    let document = render(
      source,
      &Options {
        width,
        ..Options::default()
      },
    );
    assert!(document.lines.iter().all(|line| line.width() <= width));
    let text = document.plain();
    for word in ["apple", "12", "berry", "7", "ready"] {
      assert!(text.contains(word), "width {width}: {text}");
    }
    if width == 12 {
      assert!(text.contains("Name: apple"));
    }
    if width == 80 {
      assert!(text.contains('┼'));
    }
    assert!(
      document
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| span.content.contains("ready") && span.style.bold)
    );
  }
}

#[test]
fn nested_lists_keep_their_indentation_when_wrapped() {
  let source = "- parent\n  - child with enough words to wrap across lines\n\n> quoted paragraph with enough words to wrap across lines";
  let document = render(
    source,
    &Options {
      width: 24,
      ..Options::default()
    },
  );
  let text = document.plain();
  assert!(text.contains("  - child"), "{text}");
  assert!(text.contains("    words"), "{text}");
  for line in document
    .lines
    .iter()
    .filter(|line| line.text().contains("quoted") || line.text().contains("across"))
  {
    assert!(line.width() <= 24);
  }
}

#[test]
fn raw_html_and_terminal_sequences_are_visible_text() {
  let document = render(
    "<script>alert(1)</script>\n\n`\x1b]52;c;data\x07` [link](https://example.com)",
    &Options::default(),
  );
  let text = document.plain();
  assert!(text.contains("<script>alert(1)</script>"));
  assert!(text.contains("␛]52;c;data␇"));
  assert!(!document.ansi().contains("\x1b]"));
  assert!(!document.ansi().contains('\x07'));
}
