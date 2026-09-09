use serde_json::json;

use super::{Duration, Entry, entry_lines, file_fragment, fit_subject, tool_note, tool_subject};

#[test]
fn a_call_reads_as_the_command_it_is() {
  assert_eq!(
    tool_subject("shell", &json!({"command": "git status --short"})),
    "git status --short"
  );
  assert_eq!(
    tool_subject("Read", &json!({"file_path": "/tmp/notes.md"})),
    "/tmp/notes.md"
  );
  // nothing recognisable to name, so the line is the tool alone rather than raw JSON
  assert_eq!(tool_subject("TodoWrite", &json!({"todos": []})), "");
}

#[test]
fn a_long_path_keeps_the_end_that_names_the_file() {
  let path = format!("/{}/notes.md", "deep".repeat(30));

  let subject = fit_subject(&tool_subject("Read", &json!({ "file_path": path })));

  assert!(subject.ends_with("deepdeep/notes.md"), "{subject}");
  assert!(subject.starts_with('…'), "{subject}");
}

#[test]
fn what_a_call_cost_is_what_the_line_carries() {
  let quick = Duration::from_millis(120);
  assert_eq!(tool_note("", false, quick), "120ms");
  assert_eq!(tool_note("only this", false, quick), "120ms · only this");
  assert_eq!(
    tool_note("one\ntwo\nthree", false, Duration::from_secs_f64(2.5)),
    "2.5s · 3 lines"
  );
  // a failure says why, since that is the part worth reading
  assert_eq!(
    tool_note("no such file", true, quick),
    "120ms · no such file"
  );
}

#[test]
fn a_finished_call_shows_a_few_lines_and_counts_the_rest() {
  let output = (1..=6).map(|n| format!("line {n}")).collect::<Vec<_>>();
  let mut entry = Entry::call("shell".into(), "seq 6".into());
  entry.detail = Some(output.join("\n"));

  let collapsed = rendered(&entry, false);
  assert!(
    collapsed.contains("line 1") && collapsed.contains("… +3 lines"),
    "{collapsed}"
  );
  assert!(!collapsed.contains("line 5"), "{collapsed}");

  // ctrl+o opens the whole of it
  let expanded = rendered(&entry, true);
  assert!(expanded.contains("line 6"), "{expanded}");
}

#[test]
fn a_finished_call_shows_one_line_without_expansion() {
  let mut entry = Entry::call("shell".into(), "cat config".into());
  entry.detail = Some("ready".into());

  let text = rendered(&entry, false);
  assert!(text.contains("ready"), "{text}");
}

#[test]
fn a_finished_call_does_not_show_raw_json_output() {
  let mut entry = Entry::call("read".into(), "config.json".into());
  entry.detail = Some(r#"{"name":"ainz","ok":true}"#.into());

  let collapsed = rendered(&entry, false);
  assert!(!collapsed.contains(r#""name""#), "{collapsed}");
  assert!(!collapsed.contains(r#"{"name""#), "{collapsed}");
  assert!(collapsed.contains("JSON"), "{collapsed}");

  let expanded = rendered(&entry, true);
  assert!(!expanded.contains(r#""name""#), "{expanded}");
  assert!(expanded.contains("JSON"), "{expanded}");
}

#[test]
fn a_running_call_shows_the_last_line_it_wrote() {
  let mut entry = Entry::call("shell".into(), "cargo build".into());
  entry.detail = Some("Compiling ainz\nCompiling serde\n".into());
  if let Some(tool) = entry.tool.as_mut() {
    tool.live = "Compiling serde".into();
  }

  let text = rendered(&entry, false);

  assert!(text.contains("▸"), "{text}");
  assert!(text.contains("Compiling serde"), "{text}");
  // the earlier lines wait for ctrl+o rather than crowding the line
  assert!(!text.contains("Compiling ainz"), "{text}");
}

fn rendered(entry: &Entry, expanded: bool) -> String {
  entry_lines(entry, expanded, 120)
    .iter()
    .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
    .collect()
}

#[test]
fn an_at_sign_starts_a_path_completion() {
  // the fragment is what has been typed after the @, wherever the cursor sits
  assert_eq!(
    file_fragment("look at @src/ma", 15),
    Some((8, "src/ma".into()))
  );
  // an address is not a path completion, since the @ has no space before it
  assert_eq!(file_fragment("mail me@wess.io", 15), None);
  // and neither is a finished word
  assert_eq!(file_fragment("@src/main.rs now", 16), None);
}

use super::{ChatState, EntryKind, Event, ToolState, Usage, apply_agent_event, render_transcript};
use ainz::protocol::ToolCall;
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn tool_activity_separates_assistant_messages() {
  let mut state = ChatState::default();
  state.begin_run();
  apply_agent_event(
    &mut state,
    None,
    Event::TextDelta {
      text: "Checking files.".into(),
    },
  );
  apply_agent_event(
    &mut state,
    None,
    Event::ToolStart {
      call: ToolCall {
        id: "read".into(),
        name: "read".into(),
        arguments: json!({"path": "notes"}),
      },
    },
  );
  apply_agent_event(
    &mut state,
    None,
    Event::TextDelta {
      text: "Here is the result.".into(),
    },
  );
  assert_eq!(state.primary.entries.len(), 3);
  assert_eq!(state.primary.entries[0].text, "Checking files.");
  assert_eq!(state.primary.entries[2].text, "Here is the result.");
}

#[test]
fn completion_waits_for_the_run_to_return_and_remains_visible() {
  let mut state = ChatState::default();
  state.begin_run();
  apply_agent_event(
    &mut state,
    None,
    Event::TurnEnd {
      usage: Usage::default(),
    },
  );
  assert!(state.busy());
  assert!(state.outcome.is_none());
  state.finish_run(Ok("done".into()));
  assert!(!state.busy());
  assert!(state.outcome.is_some());
  assert!(matches!(
    state.primary.entries.last().unwrap().kind,
    EntryKind::Completed
  ));
  state.begin_run();
  assert!(state.outcome.is_none());
}

#[test]
fn cancellation_and_failure_never_claim_completion() {
  for cancelled in [false, true] {
    let mut state = ChatState::default();
    state.begin_run();
    apply_agent_event(
      &mut state,
      None,
      Event::ToolStart {
        call: ToolCall {
          id: "tool".into(),
          name: "shell".into(),
          arguments: json!({}),
        },
      },
    );
    if cancelled {
      apply_agent_event(&mut state, None, Event::Cancelled);
    }
    state.finish_run(Err(anyhow::anyhow!("provider stopped")));
    assert!(state.primary.live.is_empty());
    assert!(state.primary.tools.is_empty());
    assert!(matches!(
      state.primary.entries[0].tool.as_ref().unwrap().state,
      ToolState::Stopped
    ));
    let text: String = state
      .primary
      .entries
      .iter()
      .map(|entry| rendered(entry, true))
      .collect();
    assert!(!text.contains("Completed"));
    assert!(text.contains(if cancelled { "Cancelled" } else { "Failed" }));
    assert_eq!(text.contains("provider stopped"), !cancelled);
  }
}

#[test]
fn expanded_running_output_keeps_every_line_and_long_line_endings() {
  let mut entry = Entry::call("shell".into(), "test".into());
  entry.detail = Some(format!(
    "first line\n{} tail-marker\nlast line",
    "x".repeat(200)
  ));
  entry.tool.as_mut().unwrap().live = "last line".into();
  let text = rendered(&entry, true);
  assert!(text.contains("first line"));
  assert!(text.contains("tail-marker"));
  assert!(text.contains("last line"));
}

#[test]
fn answer_formatting_preserves_code_and_unfinished_delimiters() {
  let entry = Entry::new(EntryKind::Assistant,
    "## Result\n**Fixed** the `buffer`.\n```rust\n  let literal = \"**keep**\";\n```\n**still arriving".into());
  let text = rendered(&entry, false);
  assert!(text.contains("Result"));
  assert!(!text.contains("## Result"));
  assert!(text.contains("Fixed the buffer."));
  assert!(text.contains("  let literal = \"**keep**\";"));
  assert!(text.contains("**still arriving"));
}

#[test]
fn wrapped_transcript_reaches_the_end_and_stays_put_when_reading_history() {
  let mut state = ChatState::default();
  let text = (0..60)
    .map(|n| format!("line {n}: words that need wrapping in this narrow terminal"))
    .collect::<Vec<_>>()
    .join("\n");
  state
    .primary
    .entries
    .push(Entry::new(EntryKind::Assistant, text));
  state
    .primary
    .entries
    .push(Entry::new(EntryKind::Completed, "Completed marker".into()));
  let mut terminal = Terminal::new(TestBackend::new(38, 12)).unwrap();
  terminal
    .draw(|frame| render_transcript(frame, frame.area(), &state))
    .unwrap();
  let screen = |terminal: &Terminal<TestBackend>| {
    terminal
      .backend()
      .buffer()
      .content
      .iter()
      .map(|cell| cell.symbol())
      .collect::<String>()
  };
  assert!(screen(&terminal).contains("Completed marker"));
  state.scroll_back(20);
  terminal
    .draw(|frame| render_transcript(frame, frame.area(), &state))
    .unwrap();
  let before = screen(&terminal);
  state.primary.entries.push(Entry::new(
    EntryKind::Assistant,
    "new output\nmore output".into(),
  ));
  terminal
    .draw(|frame| render_transcript(frame, frame.area(), &state))
    .unwrap();
  assert_eq!(screen(&terminal), before);
  state.scroll.set(0);
  terminal
    .draw(|frame| render_transcript(frame, frame.area(), &state))
    .unwrap();
  assert!(screen(&terminal).contains("more output"));
}
