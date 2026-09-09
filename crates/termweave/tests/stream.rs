use termweave::{Options, Stream, render};

#[test]
fn every_utf8_chunk_boundary_finishes_identically_to_whole_input() {
  let samples = [
    "# Heading\n\n**bold _nested_** with `code` and e\u{301} 👩🏽‍💻.",
    "[label][ref]\n\n[ref]: https://example.com\n\n- [x] done\n  - nested",
    "```rust\nlet text = \"**literal**\";\n```\n\n> quote",
    "| A | B |\n|---|---|\n| one | two |",
    "unfinished **bold and `code",
  ];
  for source in samples {
    for split in source.char_indices().map(|(i, _)| i).chain([source.len()]) {
      let mut stream = Stream::default();
      stream.push(&source[..split]);
      for width in [0, 1, 20, 80] {
        let snapshot = stream.snapshot(&Options {
          width,
          ..Options::default()
        });
        assert!(snapshot.lines.iter().all(|line| line.width() <= width));
      }
      stream.push(&source[split..]);
      assert_eq!(stream.source(), source);
      assert_eq!(
        stream.finish(&Options::default()),
        render(source, &Options::default())
      );
    }
  }
}

#[test]
fn unfinished_delimiters_remain_readable_and_resolve_when_closed() {
  let options = Options::default();
  let mut stream = Stream::default();
  stream.push("**hel");
  assert_eq!(stream.snapshot(&options).plain(), "**hel");
  stream.push("lo**");
  let document = stream.snapshot(&options);
  assert_eq!(document.plain(), "hello");
  assert!(document.lines[0].spans[0].style.bold);
}

#[test]
fn reference_links_revise_previous_rows() {
  let options = Options::default();
  let mut stream = Stream::default();
  stream.push("[guide][ref]");
  assert_eq!(stream.snapshot(&options).plain(), "[guide][ref]");
  stream.push("\n\n[ref]: https://example.com");
  assert_eq!(
    stream.snapshot(&options).plain(),
    "guide (https://example.com)"
  );
}

#[test]
fn open_fences_do_not_paint_a_closing_border_until_closed_or_finished() {
  let options = Options::default();
  for prefix in ["", "> ", "- "] {
    let mut stream = Stream::default();
    stream.push(&format!(
      "{prefix}```rust\n{}let x = 1;\n",
      if prefix == "- " { "  " } else { prefix }
    ));
    assert!(!stream.snapshot(&options).plain().contains('└'));
    assert!(stream.clone().finish(&options).plain().contains('└'));
    stream.push(&format!(
      "{}```",
      if prefix == "- " { "  " } else { prefix }
    ));
    assert!(
      stream.snapshot(&options).plain().contains('└'),
      "{}",
      stream.source()
    );
  }
}

#[test]
fn snapshots_can_be_reflowed_after_a_resize() {
  let mut stream = Stream::default();
  stream.push("words with **style** that wrap in a narrow terminal");
  let wide = stream.snapshot(&Options {
    width: 80,
    ..Options::default()
  });
  let narrow = stream.snapshot(&Options {
    width: 12,
    ..Options::default()
  });
  assert!(narrow.lines.len() > wide.lines.len());
  assert!(narrow.lines.iter().all(|line| line.width() <= 12));
}
