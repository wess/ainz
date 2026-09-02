use std::process::Stdio;

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

  let output = Command::new(env!("CARGO_BIN_EXE_agentx"))
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
  let mut child = Command::new(env!("CARGO_BIN_EXE_agentx"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_agentx"))
      .args(args)
      .env("AGENTX_CONFIG", &config)
      .output()
      .await
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  }

  let output = Command::new(env!("CARGO_BIN_EXE_agentx"))
    .args(["providers", "list", "--json"])
    .env("AGENTX_CONFIG", &config)
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
    let output = Command::new(env!("CARGO_BIN_EXE_agentx"))
      .args(args)
      .env("AGENTX_CONFIG", &config)
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
  let mut child = Command::new(env!("CARGO_BIN_EXE_agentx"))
    .env("AGENTX_CONFIG", &config)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  child
    .stdin
    .take()
    .unwrap()
    .write_all(b"4\ndemo\nhttp://127.0.0.1:9999/v1\n\ntiny\n/exit\n")
    .await
    .unwrap();
  let output = child.wait_with_output().await.unwrap();

  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("AgentX setup"));
  assert!(stdout.contains("configured demo · tiny"));
  assert!(stdout.contains("AgentX · demo · tiny"));
  let saved = tokio::fs::read_to_string(config).await.unwrap();
  assert!(saved.contains("provider = \"demo\""));
  assert!(saved.contains("model = \"tiny\""));
}

#[tokio::test]
async fn mcp_commands_persist_a_synapse_compatible_registration() {
  let dir = tempfile::tempdir().unwrap();
  let profile = dir.path().join("mcp.toml");
  let added = Command::new(env!("CARGO_BIN_EXE_agentx"))
    .args([
      "mcp",
      "add",
      "synapse",
      "--required",
      "--",
      "/opt/synapse",
      "mcp",
    ])
    .env("AGENTX_MCP_PROFILE", &profile)
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

  let removed = Command::new(env!("CARGO_BIN_EXE_agentx"))
    .args(["mcp", "remove", "synapse"])
    .env("AGENTX_MCP_PROFILE", &profile)
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
async fn the_agentx_rename_carries_forward_existing_user_configuration() {
  let dir = tempfile::tempdir().unwrap();
  let root = config_root(dir.path());
  let legacy = root.join("struts");
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

  let output = Command::new(env!("CARGO_BIN_EXE_agentx"))
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
  assert!(root.join("agentx/config.toml").exists());
  assert!(root.join("agentx/mcp.toml").exists());
}
