use std::process::Stdio;

use ainz::{
  Session, SessionStore,
  protocol::{Message, Role},
};
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
  process::Command,
};

fn config_root(home: &std::path::Path) -> std::path::PathBuf {
  if cfg!(target_os = "macos") {
    home.join("Library/Application Support")
  } else {
    home.join(".config")
  }
}

#[tokio::test]
async fn ask_json_runs_end_to_end() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = vec![0; 16 * 1024];
    let _ = socket.read(&mut request).await.unwrap();
    let body = r#"{"choices":[{"message":{"content":"smoke","tool_calls":[]}}],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
      body.len(),
      body,
    );
    socket.write_all(response.as_bytes()).await.unwrap();
  });

  let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args([
      "--model",
      "test",
      "--endpoint",
      &format!("http://{address}"),
      "ask",
      "--json",
      "--no-save",
      "hello",
    ])
    .output()
    .await
    .unwrap();

  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(
    stdout
      .lines()
      .any(|line| line.contains(r#""text":"smoke""#))
  );
  assert!(
    stdout
      .lines()
      .any(|line| line.contains(r#""type":"turn_end""#))
  );
  server.await.unwrap();
}

#[tokio::test]
async fn rpc_mode_keeps_a_session_and_returns_json_rpc_responses() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = vec![0; 16 * 1024];
    let _ = socket.read(&mut request).await.unwrap();
    let body = r#"{"choices":[{"message":{"content":"rpc result","tool_calls":[]}}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
      body.len(),
      body,
    );
    socket.write_all(response.as_bytes()).await.unwrap();
  });
  let mut child = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args([
      "--model",
      "test",
      "--endpoint",
      &format!("http://{address}"),
      "rpc",
      "--no-save",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  let mut stdin = child.stdin.take().unwrap();
  let stdout = child.stdout.take().unwrap();
  let mut lines = tokio::io::BufReader::new(stdout).lines();
  stdin
    .write_all(
      b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"prompt\",\"params\":{\"prompt\":\"hello\"}}\n",
    )
    .await
    .unwrap();
  let response = loop {
    let line = lines.next_line().await.unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    if value["id"] == 1 {
      break value;
    }
  };
  assert_eq!(response["result"]["output"], "rpc result");
  stdin
    .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"shutdown\"}\n")
    .await
    .unwrap();
  drop(stdin);
  let output = child.wait_with_output().await.unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  server.await.unwrap();
}

#[tokio::test]
async fn provider_and_model_commands_persist_selection() {
  let dir = tempfile::tempdir().unwrap();
  let config = dir.path().join("config.toml");
  for args in [
    vec!["providers", "add", "local", "--preset", "ollama"],
    vec!["models", "add", "local", "tiny"],
    vec!["providers", "use", "local", "tiny"],
  ] {
    let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
      .args(args)
      .env("AINZ_CONFIG", &config)
      .output()
      .await
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  }

  let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args(["providers", "list", "--json"])
    .env("AINZ_CONFIG", &config)
    .output()
    .await
    .unwrap();
  let providers: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

  assert_eq!(providers[0]["name"], "local");
  assert_eq!(providers[0]["active"], true);
  assert_eq!(providers[0]["models"][0], "tiny");
}

#[tokio::test]
async fn switching_provider_selects_one_of_its_models() {
  let dir = tempfile::tempdir().unwrap();
  let config = dir.path().join("config.toml");
  for args in [
    vec![
      "providers",
      "add",
      "first",
      "--endpoint",
      "http://localhost/one",
      "--known-model",
      "one",
    ],
    vec![
      "providers",
      "add",
      "second",
      "--endpoint",
      "http://localhost/two",
      "--known-model",
      "two",
    ],
    vec!["providers", "use", "first", "one"],
    vec!["providers", "use", "second"],
  ] {
    let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
      .args(args)
      .env("AINZ_CONFIG", &config)
      .output()
      .await
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  }

  let saved = tokio::fs::read_to_string(config).await.unwrap();
  assert!(saved.contains("provider = \"second\""));
  assert!(saved.contains("model = \"two\""));
}

#[tokio::test]
async fn empty_config_starts_the_interactive_setup() {
  let dir = tempfile::tempdir().unwrap();
  let config = dir.path().join("config.toml");
  let mut child = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("AINZ_CONFIG", &config)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  child
    .stdin
    .take()
    .unwrap()
    .write_all(b"5\ndemo\nhttp://127.0.0.1:9999/v1\n\ntiny\n/exit\n")
    .await
    .unwrap();
  let output = child.wait_with_output().await.unwrap();

  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("Ainz setup"));
  assert!(stdout.contains("configured demo · tiny"));
  assert!(stdout.contains("Ainz · demo · tiny"));
  let saved = tokio::fs::read_to_string(config).await.unwrap();
  assert!(saved.contains("provider = \"demo\""));
  assert!(saved.contains("model = \"tiny\""));
}

#[tokio::test]
async fn mcp_commands_persist_a_synapse_compatible_registration() {
  let dir = tempfile::tempdir().unwrap();
  let profile = dir.path().join("mcp.toml");
  let added = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args([
      "mcp",
      "add",
      "synapse",
      "--required",
      "--",
      "/opt/synapse",
      "mcp",
    ])
    .env("AINZ_MCP_PROFILE", &profile)
    .output()
    .await
    .unwrap();
  assert!(
    added.status.success(),
    "{}",
    String::from_utf8_lossy(&added.stderr)
  );
  let text = tokio::fs::read_to_string(&profile).await.unwrap();
  assert!(text.contains("command = \"/opt/synapse\""));
  assert!(text.contains("args = [\"mcp\"]"));
  assert!(text.contains("required = true"));

  let removed = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args(["mcp", "remove", "synapse"])
    .env("AINZ_MCP_PROFILE", &profile)
    .output()
    .await
    .unwrap();
  assert!(removed.status.success());
  assert!(
    !tokio::fs::read_to_string(profile)
      .await
      .unwrap()
      .contains("synapse")
  );
}

#[tokio::test]
async fn the_ainz_rename_carries_forward_existing_user_configuration() {
  let dir = tempfile::tempdir().unwrap();
  let root = config_root(dir.path());
  let legacy = root.join("agentx");
  tokio::fs::create_dir_all(&legacy).await.unwrap();
  tokio::fs::write(legacy.join("config.toml"), "model = \"carried-forward\"\n")
    .await
    .unwrap();
  tokio::fs::write(
    legacy.join("mcp.toml"),
    "[servers.synapse]\ntransport = \"stdio\"\ncommand = \"/opt/synapse\"\nargs = [\"mcp\"]\nenabled = true\nrequired = true\ntimeout_ms = 30000\n",
  )
  .await
  .unwrap();

  let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args(["mcp", "--json"])
    .env("HOME", dir.path())
    .env("XDG_CONFIG_HOME", dir.path().join(".config"))
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(String::from_utf8_lossy(&output.stdout).contains("/opt/synapse"));
  assert!(root.join("ainz/config.toml").exists());
  assert!(root.join("ainz/mcp.toml").exists());
}

#[tokio::test]
async fn stored_memory_reaches_the_system_prompt() {
  let home = tempfile::tempdir().unwrap();
  let workspace = tempfile::tempdir().unwrap();
  let config = config_root(home.path()).join("ainz/config.toml");
  tokio::fs::create_dir_all(config.parent().unwrap())
    .await
    .unwrap();

  let stored = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("HOME", home.path())
    .env("AINZ_CONFIG", &config)
    .args([
      "--workspace",
      workspace.path().to_str().unwrap(),
      "memory",
      "add",
      "the staging database is named orbit",
    ])
    .output()
    .await
    .unwrap();
  assert!(
    stored.status.success(),
    "{}",
    String::from_utf8_lossy(&stored.stderr)
  );

  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = vec![0; 64 * 1024];
    let read = socket.read(&mut request).await.unwrap();
    let body = r#"{"choices":[{"message":{"content":"ok","tool_calls":[]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
      body.len(),
      body,
    );
    socket.write_all(response.as_bytes()).await.unwrap();
    String::from_utf8_lossy(&request[..read]).into_owned()
  });

  let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("HOME", home.path())
    .env("AINZ_CONFIG", &config)
    .args([
      "--workspace",
      workspace.path().to_str().unwrap(),
      "--model",
      "test",
      "--endpoint",
      &format!("http://{address}"),
      "ask",
      "--json",
      "--no-save",
      "what is the staging database called",
    ])
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );

  let request = server.await.unwrap();
  assert!(
    request.contains("the staging database is named orbit"),
    "recalled memory is missing from the request: {request}"
  );
  // and the tools that memory brings with it are offered to the model
  assert!(request.contains("\"name\":\"memory\""), "{request}");
  assert!(request.contains("\"name\":\"sessions\""), "{request}");
}

#[tokio::test]
async fn import_offers_what_other_tools_have_and_copies_it_once() {
  let home = tempfile::tempdir().unwrap();
  let workspace = tempfile::tempdir().unwrap();
  let config = config_root(home.path()).join("ainz/config.toml");
  let profile = config_root(home.path()).join("ainz/mcp.toml");
  tokio::fs::create_dir_all(config.parent().unwrap())
    .await
    .unwrap();
  tokio::fs::write(
    home.path().join(".claude.json"),
    format!(
      r#"{{"mcpServers": {{"files": {{"command": "server", "args": ["--stdio"]}},
          "remote": {{"type": "http", "url": "https://example/mcp", "headers": {{"Authorization": "Bearer x"}}}}}},
        "projects": {{"{}": {{"mcpServers": {{"here": {{"command": "local"}}}}}}}}}}"#,
      workspace.path().display()
    ),
  )
  .await
  .unwrap();
  tokio::fs::create_dir_all(home.path().join(".codex"))
    .await
    .unwrap();
  tokio::fs::write(
    home.path().join(".codex/config.toml"),
    "[mcp_servers.codexy]\ncommand = \"run\"\nargs = [\"mcp\"]\n",
  )
  .await
  .unwrap();

  let listing = ainz(&home, &config, &profile, &workspace, &["import", "--json"]).await;
  let rows: serde_json::Value = serde_json::from_str(&listing).unwrap();
  let names: Vec<&str> = rows
    .as_array()
    .unwrap()
    .iter()
    .map(|row| row["name"].as_str().unwrap())
    .collect();
  assert_eq!(names, ["codexy", "files", "here", "remote"]);
  // an inline Authorization header is a copied secret, and the screen says so before it moves
  let remote = &rows.as_array().unwrap()[3];
  assert_eq!(remote["credentials"], true);
  assert_eq!(rows.as_array().unwrap()[1]["credentials"], false);
  assert!(
    rows
      .as_array()
      .unwrap()
      .iter()
      .all(|row| row["present"] == false)
  );

  let imported = ainz(&home, &config, &profile, &workspace, &["import", "--all"]).await;
  assert_eq!(imported.lines().count(), 4, "{imported}");

  let written = tokio::fs::read_to_string(&profile).await.unwrap();
  assert!(written.contains("[servers.files]"), "{written}");
  assert!(written.contains("[servers.here]"), "{written}");
  assert!(written.contains("command = \"run\""), "{written}");
  // nothing imported is required, so a server that will not start cannot block a session
  assert!(!written.contains("required = true"), "{written}");

  let again = ainz(&home, &config, &profile, &workspace, &["import", "--json"]).await;
  let rows: serde_json::Value = serde_json::from_str(&again).unwrap();
  assert!(
    rows
      .as_array()
      .unwrap()
      .iter()
      .all(|row| row["present"] == true)
  );
}

async fn ainz(
  home: &tempfile::TempDir,
  config: &std::path::Path,
  profile: &std::path::Path,
  workspace: &tempfile::TempDir,
  args: &[&str],
) -> String {
  let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("HOME", home.path())
    .env("AINZ_CONFIG", config)
    .env("AINZ_MCP_PROFILE", profile)
    .args(["--workspace", workspace.path().to_str().unwrap()])
    .args(args)
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn sessions_export_writes_the_active_path_and_defaults_to_the_latest_session() {
  let home = tempfile::tempdir().unwrap();
  let workspace = tempfile::tempdir().unwrap();
  let store = SessionStore::new(config_root(home.path()).join("ainz/sessions"));

  // main.rs canonicalizes --workspace before comparing it against a stored session's
  // workspace, and a tempdir path is often a symlink (e.g. /tmp -> /private/tmp on macOS)
  let mut session = Session::new(workspace.path().canonicalize().unwrap());
  let root = session.append(Message::text(Role::User, "what broke the deploy"));
  // rewound away below; export must not resurrect it
  session.append(Message::text(Role::Assistant, "abandoned guess about DNS"));
  session.checkout(Some(root)).unwrap();
  session.append(Message::text(
    Role::Assistant,
    "checking the certificate chain",
  ));
  store.save(&session).await.unwrap();

  let output = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("HOME", home.path())
    .args([
      "--workspace",
      workspace.path().to_str().unwrap(),
      "sessions",
      "export",
    ])
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains(&format!("# Session {}", session.id)));
  assert!(stdout.contains("what broke the deploy"));
  assert!(stdout.contains("checking the certificate chain"));
  assert!(!stdout.contains("abandoned guess about DNS"));

  // --out writes the same Markdown to a file instead of stdout
  let out_path = workspace.path().join("export.md");
  let written = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("HOME", home.path())
    .args([
      "--workspace",
      workspace.path().to_str().unwrap(),
      "sessions",
      "export",
      "--out",
      out_path.to_str().unwrap(),
    ])
    .output()
    .await
    .unwrap();
  assert!(
    written.status.success(),
    "{}",
    String::from_utf8_lossy(&written.stderr)
  );
  assert!(written.stdout.is_empty());
  let file_contents = tokio::fs::read_to_string(&out_path).await.unwrap();
  assert!(file_contents.contains("checking the certificate chain"));

  // plain `sessions` and `sessions --json` still list rather than export
  let listed = Command::new(env!("CARGO_BIN_EXE_ainz"))
    .env("HOME", home.path())
    .args([
      "--workspace",
      workspace.path().to_str().unwrap(),
      "sessions",
      "--json",
    ])
    .output()
    .await
    .unwrap();
  assert!(listed.status.success());
  let rows: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
  assert_eq!(rows[0]["id"], session.id.to_string());
}
