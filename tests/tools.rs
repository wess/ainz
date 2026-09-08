use ainz::tool::{ToolContext, ToolSet, builtins};
use serde_json::json;

#[tokio::test]
async fn builtins_edit_inside_the_workspace() {
  let temp = tempfile::tempdir().unwrap();
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
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
  let context = ToolContext::new(workspace.path().into(), uuid::Uuid::nil(), 1024);
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
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
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
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
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
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
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
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 4096);
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

#[tokio::test]
async fn the_shell_reports_output_while_the_command_is_still_running() {
  let temp = tempfile::tempdir().unwrap();
  let (events, mut received) = ainz::EventSink::channel();
  let mut context = ToolContext::new(temp.path().to_path_buf(), uuid::Uuid::nil(), 8192);
  context.progress = Some((events, "call-1".into()));
  let tool = builtins()
    .into_iter()
    .find(|tool| tool.spec().name == "shell")
    .unwrap();

  let run = tokio::spawn(async move {
    tool
      .execute(
        &context,
        json!({"command": "echo first; sleep 0.4; echo second"}),
      )
      .await
      .unwrap()
  });

  // the first line has to arrive before the command that writes the second one is done
  let first = tokio::time::timeout(std::time::Duration::from_secs(2), async {
    loop {
      match received.recv().await {
        Some(ainz::Event::ToolDelta { id, text }) => {
          assert_eq!(id, "call-1");
          return text;
        }
        Some(_) => continue,
        None => panic!("the sink closed before a delta arrived"),
      }
    }
  })
  .await
  .expect("no delta while the command was running");
  assert_eq!(first.trim(), "first");
  assert!(!run.is_finished(), "the command had already finished");

  let output = run.await.unwrap();
  assert!(
    output.contains("first") && output.contains("second"),
    "{output}"
  );
  assert!(output.contains("[exit 0]"), "{output}");
}

#[tokio::test]
async fn fetch_refuses_what_is_not_the_web() {
  let temp = tempfile::tempdir().unwrap();
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 4096);
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let fetch = tools.get("fetch").unwrap().clone();

  for url in [
    "file:///etc/passwd",
    "http://localhost:9/",
    "http://169.254.169.254/latest/meta-data",
  ] {
    let error = fetch
      .execute(&context, json!({ "url": url }))
      .await
      .unwrap_err();
    assert!(
      format!("{error:#}").contains("fetch"),
      "{url} was not refused: {error:#}"
    );
  }
}

#[tokio::test]
async fn shell_output_and_progress_stay_bounded() {
  let temp = tempfile::tempdir().unwrap();
  let (events, mut received) = ainz::EventSink::channel();
  let mut context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
  context.progress = Some((events, "bounded".into()));
  let shell = builtins()
    .into_iter()
    .find(|tool| tool.spec().name == "shell")
    .unwrap();
  let output = shell
    .execute(
      &context,
      json!({"command":"head -c 2097152 /dev/zero", "timeout_ms":5000}),
    )
    .await
    .unwrap();
  assert!(output.len() <= 1024 + 32);
  assert!(output.contains("truncated"));
  let mut bytes = 0;
  while let Ok(event) = received.try_recv() {
    if let ainz::Event::ToolDelta { text, .. } = event {
      bytes += text.len();
    }
  }
  assert!(bytes <= 1024 + 32, "unbounded progress: {bytes}");
}

#[tokio::test]
async fn read_refuses_a_line_larger_than_its_transfer_limit() {
  let temp = tempfile::tempdir().unwrap();
  tokio::fs::write(temp.path().join("long"), "x".repeat(128 * 1024))
    .await
    .unwrap();
  let read = builtins()
    .into_iter()
    .find(|tool| tool.spec().name == "read")
    .unwrap();
  let error = read
    .execute(
      &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
      json!({"path":"long"}),
    )
    .await
    .unwrap_err();
  assert!(error.to_string().contains("transfer limit"));
}

#[tokio::test]
async fn shell_progress_preserves_characters_split_across_reads() {
  let temp = tempfile::tempdir().unwrap();
  let (events, mut received) = ainz::EventSink::channel();
  let mut context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
  context.progress = Some((events, "unicode".into()));
  let shell = builtins()
    .into_iter()
    .find(|tool| tool.spec().name == "shell")
    .unwrap();
  let output = shell
    .execute(
      &context,
      json!({"command": "printf '\\303'; sleep 0.1; printf '\\251'"}),
    )
    .await
    .unwrap();
  assert!(output.starts_with("é\n[exit 0]"));
  let mut progress = String::new();
  while let Ok(ainz::Event::ToolDelta { text, .. }) = received.try_recv() {
    progress.push_str(&text);
  }
  assert_eq!(progress, "é");
}
