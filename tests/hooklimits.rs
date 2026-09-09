use std::{collections::BTreeMap, time::Duration};

use ainz::{EventSink, HookDef, HookRunner, protocol::ToolCall};
use serde_json::json;

#[tokio::test]
async fn large_hook_output_is_drained_while_input_is_written_and_retention_is_bounded() {
  let root = tempfile::tempdir().unwrap();
  let hook = HookRunner::new(BTreeMap::from([("pre_tool".into(), vec![HookDef {
    command: vec!["python3".into(), "-c".into(),
      "import sys; sys.stdout.write('o' * 2097152); sys.stdout.flush(); sys.stderr.write('e' * 2097152); sys.stderr.flush(); sys.stdin.buffer.read(); sys.exit(1)".into(),
    ], matcher: None,
  }])]));
  let call = ToolCall {
    id: "read".into(),
    name: "read".into(),
    arguments: json!({"large": "x".repeat(256 * 1024)}),
  };
  let error = tokio::time::timeout(
    Duration::from_secs(3),
    hook.pre_tool(root.path(), uuid::Uuid::nil(), &call, &EventSink::default()),
  )
  .await
  .unwrap()
  .unwrap_err();
  assert!(error.contains("[hook output truncated]"));
  assert!(error.len() < 66 * 1024);
}

#[tokio::test]
async fn relative_hooks_execute_in_the_selected_workspace() {
  let root = tempfile::tempdir().unwrap();
  tokio::fs::write(
    root.path().join("hook.sh"),
    "cat >/dev/null\nprintf ran > marker\n",
  )
  .await
  .unwrap();
  let hook = HookRunner::new(BTreeMap::from([(
    "pre_tool".into(),
    vec![HookDef {
      command: vec!["/bin/sh".into(), "./hook.sh".into()],
      matcher: None,
    }],
  )]));
  let call = ToolCall {
    id: "read".into(),
    name: "read".into(),
    arguments: json!({}),
  };
  hook
    .pre_tool(root.path(), uuid::Uuid::nil(), &call, &EventSink::default())
    .await
    .unwrap();
  assert_eq!(
    tokio::fs::read_to_string(root.path().join("marker"))
      .await
      .unwrap(),
    "ran"
  );
}
