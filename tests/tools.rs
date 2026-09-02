use agentx::tool::{ToolContext, ToolSet, builtins};
use serde_json::json;

#[tokio::test]
async fn builtins_edit_inside_the_workspace() {
  let temp = tempfile::tempdir().unwrap();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 1024,
  };
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();

  tools
    .get("write")
    .unwrap()
    .execute(
      &context,
      json!({
        "path": "src/file.txt", "content": "before"
      }),
    )
    .await
    .unwrap();
  tools
    .get("edit")
    .unwrap()
    .execute(
      &context,
      json!({
        "path": "src/file.txt", "old": "before", "new": "after"
      }),
    )
    .await
    .unwrap();
  let output = tools
    .get("read")
    .unwrap()
    .execute(
      &context,
      json!({
        "path": "src/file.txt"
      }),
    )
    .await
    .unwrap();

  assert_eq!(output, "after");
}

#[cfg(unix)]
#[tokio::test]
async fn builtins_reject_symlinks_that_escape_the_workspace() {
  use std::os::unix::fs::symlink;

  let workspace = tempfile::tempdir().unwrap();
  let outside = tempfile::tempdir().unwrap();
  tokio::fs::write(outside.path().join("secret.txt"), "secret")
    .await
    .unwrap();
  symlink(outside.path(), workspace.path().join("escape")).unwrap();
  let context = ToolContext {
    workspace: workspace.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 1024,
  };
  let tools = builtins();
  let read = tools
    .iter()
    .find(|tool| tool.spec().name == "read")
    .unwrap();
  let error = read
    .execute(&context, json!({"path": "escape/secret.txt"}))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("path escapes the workspace"));
  let write = tools
    .iter()
    .find(|tool| tool.spec().name == "write")
    .unwrap();
  let error = write
    .execute(&context, json!({"path": "escape/new.txt", "content": "no"}))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("path escapes the workspace"));
}

#[tokio::test]
async fn builtins_reject_paths_outside_the_workspace() {
  let temp = tempfile::tempdir().unwrap();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 1024,
  };
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let error = tools
    .get("read")
    .unwrap()
    .execute(&context, json!({"path": "../secret"}))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("escapes"));
}

#[tokio::test]
async fn search_treats_a_leading_dash_as_pattern_text() {
  if std::process::Command::new("rg")
    .arg("--version")
    .output()
    .is_err()
  {
    eprintln!("ripgrep is not installed; skipping");
    return;
  }
  let temp = tempfile::tempdir().unwrap();
  tokio::fs::write(temp.path().join("notes.txt"), "keep --pre out of flags\n")
    .await
    .unwrap();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 1024,
  };
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let output = tools
    .get("search")
    .unwrap()
    .execute(&context, json!({"query": "--pre"}))
    .await
    .unwrap();
  assert!(output.contains("notes.txt:1:keep --pre out of flags"));
}

#[tokio::test]
async fn shell_timeout_takes_the_whole_process_tree_down() {
  let temp = tempfile::tempdir().unwrap();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 1024,
  };
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let marker = temp.path().join("marker");
  let command = format!("(sleep 1; touch {}) & sleep 5", marker.display());
  let started = std::time::Instant::now();
  let error = tools
    .get("shell")
    .unwrap()
    .execute(&context, json!({"command": command, "timeout_ms": 200}))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("timed out"));
  assert!(started.elapsed() < std::time::Duration::from_secs(3));
  tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
  assert!(!marker.exists(), "background child survived the timeout");
}

#[tokio::test]
async fn read_returns_the_requested_window_of_lines() {
  let temp = tempfile::tempdir().unwrap();
  let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
  tokio::fs::write(temp.path().join("big.txt"), text)
    .await
    .unwrap();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let output = tools
    .get("read")
    .unwrap()
    .execute(
      &context,
      json!({"path": "big.txt", "offset": 10, "limit": 3}),
    )
    .await
    .unwrap();
  assert_eq!(output, "line 10\nline 11\nline 12");
}
