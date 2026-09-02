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
