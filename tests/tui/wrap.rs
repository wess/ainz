use super::{Entry, EntryKind, entry_lines};

#[test]
fn irc_messages_keep_the_nick_and_text_together_with_hanging_wraps() {
  for kind in [EntryKind::User, EntryKind::Assistant] {
    let mut entry = Entry::new(
      kind,
      "first second third fourth fifth sixth seventh eighth".into(),
    );
    entry.at = "12:34".into();
    let lines = entry_lines(&entry, false, 34);
    let rows: Vec<String> = lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect()
      })
      .collect();
    let nick = if matches!(kind, EntryKind::User) {
      "<you>"
    } else {
      "<Ainz>"
    };
    assert!(rows[0].contains(&format!("{nick} first")), "{rows:?}");
    assert!(rows.len() > 1);
    assert!(
      rows[1..]
        .iter()
        .all(|row| row.starts_with("              ")),
      "{rows:?}"
    );
    assert!(lines.iter().all(|line| line.width() <= 34));
    assert!(rows.last().unwrap().ends_with("eighth"));
  }
}

#[test]
fn wrapped_markdown_keeps_emphasis_and_unicode_in_terminal_cells() {
  let mut entry = Entry::new(
    EntryKind::Assistant,
    "**界界界界界界界界界界界界**\n`e\u{301} code`".into(),
  );
  entry.at = "12:34".into();
  let lines = entry_lines(&entry, false, 24);
  assert!(lines.iter().all(|line| line.width() <= 24));
  let mut ideographs = 0;
  for line in &lines {
    for span in &line.spans {
      let count = span.content.matches('界').count();
      if count > 0 {
        assert!(
          span
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
        );
      }
      ideographs += count;
    }
  }
  assert_eq!(ideographs, 12);
  assert!(
    lines
      .iter()
      .flat_map(|line| &line.spans)
      .any(|span| span.content.contains("e\u{301}"))
  );
}

#[test]
fn tiny_buffers_keep_room_for_the_message() {
  let entry = Entry::new(EntryKind::User, "some words".into());
  for width in 1..20 {
    let lines = entry_lines(&entry, false, width);
    assert!(
      lines.iter().all(|line| line.width() <= width),
      "width {width}: {lines:?}"
    );
  }
}

#[test]
fn streaming_code_stays_open_until_the_message_finishes() {
  use super::{ChatState, Event, Usage, apply_agent_event};
  let mut state = ChatState::default();
  state.begin_run();
  apply_agent_event(
    &mut state,
    None,
    Event::TextDelta {
      text: "```rust\nlet x = 1;".into(),
    },
  );
  let text = |entry: &Entry| {
    entry_lines(entry, false, 80)
      .iter()
      .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
      .collect::<String>()
  };
  assert!(!text(&state.primary.entries[0]).contains('└'));
  apply_agent_event(
    &mut state,
    None,
    Event::TurnEnd {
      usage: Usage::default(),
    },
  );
  assert!(text(&state.primary.entries[0]).contains('└'));
  assert!(state.busy());
}
