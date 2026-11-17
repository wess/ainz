//! The shell tool under `AINZ_SANDBOX`. One test in its own binary, because the policy is read
//! from the environment and no other test should run beside it under a sandbox it did not ask
//! for.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::path::{Path, PathBuf};

use ainz::tool::{ToolContext, builtins};
use serde_json::json;

#[tokio::test]
async fn the_shell_tool_writes_the_workspace_and_nothing_outside_it() {
  let confiner = if cfg!(target_os = "macos") {
    ainz::sandbox::SANDBOX_EXEC
  } else {
    ainz::sandbox::BWRAP
  };
  if !Path::new(confiner).exists() {
    return;
  }
  // SAFETY: this binary holds this one test, so nothing else reads the environment meanwhile
  unsafe { std::env::set_var(ainz::sandbox::VARIABLE, "workspace") };
  let workspace = tempfile::tempdir().unwrap();
  let context = ToolContext::new(workspace.path().into(), uuid::Uuid::nil(), 4096);
  let shell = builtins()
    .into_iter()
    .find(|tool| tool.spec().name == "shell")
    .unwrap();
  let outside = PathBuf::from(std::env::var("HOME").unwrap()).join(format!(
    ".ainz-confined-probe-{}",
    uuid::Uuid::now_v7().simple()
  ));

  let inside = shell
    .execute(&context, json!({"command": "echo kept > inside.txt"}))
    .await
    .unwrap();
  let escaped = shell
    .execute(
      &context,
      json!({"command": format!("echo escaped > {}", outside.display())}),
    )
    .await
    .unwrap();

  assert!(inside.ends_with("[exit 0]"), "{inside}");
  assert!(workspace.path().join("inside.txt").exists());
  assert!(!escaped.ends_with("[exit 0]"), "{escaped}");
  assert!(!outside.exists(), "a write to $HOME escaped the sandbox");
}
