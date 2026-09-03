use std::collections::BTreeMap;

use ainz::{EventSink, HookDef, HookRunner, protocol::ToolCall};
use serde_json::{Value, json};
use uuid::Uuid;

fn hooks(event: &str, def: HookDef) -> HookRunner {
  multi(event, vec![def])
}

fn multi(event: &str, defs: Vec<HookDef>) -> HookRunner {
  let mut map = BTreeMap::new();
  map.insert(event.to_string(), defs);
  HookRunner::new(map)
}

fn call(name: &str) -> ToolCall {
  ToolCall {
    id: "call-1".into(),
    name: name.into(),
    arguments: json!({"path": "x"}),
  }
}

#[tokio::test]
async fn session_start_runs_and_receives_the_payload_on_stdin() {
  let temp = tempfile::tempdir().unwrap();
  let captured = temp.path().join("stdin.json");
  let runner = hooks(
    "session_start",
    HookDef {
      command: vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("cat > {}", captured.display()),
      ],
      matcher: None,
    },
  );
  let session_id = Uuid::now_v7();

  runner
    .session_start(temp.path(), session_id, &EventSink::default())
    .await;

  let text = tokio::fs::read_to_string(&captured).await.unwrap();
  let payload: Value = serde_json::from_str(&text).unwrap();
  assert_eq!(payload["event"], "session_start");
  assert_eq!(payload["session_id"], session_id.to_string());
  assert_eq!(payload["workspace"], temp.path().display().to_string());
  assert!(payload.get("tool").is_none());
}

#[tokio::test]
async fn a_nonzero_pre_tool_hook_blocks_the_call() {
  let temp = tempfile::tempdir().unwrap();
  let runner = hooks(
    "pre_tool",
    HookDef {
      command: vec!["/usr/bin/false".into()],
      matcher: None,
    },
  );

  let result = runner
    .pre_tool(
      temp.path(),
      Uuid::now_v7(),
      &call("shell"),
      &EventSink::default(),
    )
    .await;

  assert!(result.is_err());
}

#[tokio::test]
async fn a_failing_post_tool_hook_does_not_block() {
  let temp = tempfile::tempdir().unwrap();
  let runner = hooks(
    "post_tool",
    HookDef {
      command: vec!["/usr/bin/false".into()],
      matcher: None,
    },
  );
  let (events, mut rx) = EventSink::channel();

  // post_tool has no way to signal failure back to the caller; the only observable proof it
  // did not block is that it returns at all, and that the failure still surfaced somewhere
  runner
    .post_tool(
      temp.path(),
      Uuid::now_v7(),
      &call("shell"),
      "tool output",
      false,
      &events,
    )
    .await;
  drop(events);

  let event = rx.recv().await.expect("a failure was reported");
  match event {
    ainz::Event::Error { message } => assert!(message.contains("post_tool")),
    other => panic!("expected an Error event, got {other:?}"),
  }
}

#[tokio::test]
async fn a_nonmatching_matcher_does_not_run_the_hook() {
  let temp = tempfile::tempdir().unwrap();
  let marker = temp.path().join("ran");
  // one hook that would prove it ran if it ran, and one that always matches, so a mismatch on
  // the first is distinguishable from the whole event being skipped
  let runner = multi(
    "pre_tool",
    vec![
      HookDef {
        command: vec![
          "/bin/sh".into(),
          "-c".into(),
          format!("touch {}", marker.display()),
        ],
        matcher: Some("edit".into()),
      },
      HookDef {
        command: vec!["/usr/bin/true".into()],
        matcher: None,
      },
    ],
  );

  let result = runner
    .pre_tool(
      temp.path(),
      Uuid::now_v7(),
      &call("shell"),
      &EventSink::default(),
    )
    .await;

  assert!(result.is_ok());
  assert!(!marker.exists());
}

#[tokio::test]
async fn no_hooks_configured_means_nothing_runs() {
  let temp = tempfile::tempdir().unwrap();
  let runner = HookRunner::default();
  let (events, mut rx) = EventSink::channel();

  runner
    .session_start(temp.path(), Uuid::now_v7(), &events)
    .await;
  let blocked = runner
    .pre_tool(temp.path(), Uuid::now_v7(), &call("shell"), &events)
    .await;
  runner
    .post_tool(
      temp.path(),
      Uuid::now_v7(),
      &call("shell"),
      "output",
      false,
      &events,
    )
    .await;
  runner
    .session_end(temp.path(), Uuid::now_v7(), Some("done"), false, &events)
    .await;
  drop(events);

  assert!(blocked.is_ok());
  assert!(rx.try_recv().is_err());
}
