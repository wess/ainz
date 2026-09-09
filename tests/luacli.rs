#![cfg(all(feature = "cli", feature = "lua"))]

use std::{path::Path, time::Duration};

use serde_json::{Value, json};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::{TcpListener, TcpStream},
  process::Command,
};

fn cli(home: &Path) -> Command {
  let mut command = Command::new(env!("CARGO_BIN_EXE_ainz"));
  command
    .env_clear()
    .env("HOME", home)
    .env("XDG_CONFIG_HOME", home.join("config"))
    .env("XDG_DATA_HOME", home.join("data"))
    .env("AINZ_CONFIG", home.join("config.toml"))
    .current_dir(home)
    .kill_on_drop(true);
  command
}

async fn request(socket: &mut TcpStream) -> Value {
  let mut bytes = Vec::new();
  loop {
    let mut chunk = [0; 8192];
    let read = socket.read(&mut chunk).await.unwrap();
    assert!(read > 0);
    bytes.extend_from_slice(&chunk[..read]);
    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
      let header = String::from_utf8_lossy(&bytes[..end]);
      let length: usize = header
        .lines()
        .find_map(|line| {
          line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(|value| value.trim().parse().unwrap())
        })
        .unwrap();
      if bytes.len() >= end + 4 + length {
        return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
      }
    }
  }
}

#[tokio::test]
async fn approved_lua_fixture_runs_through_the_cli_tool_loop() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/plugins/greeting");
  tokio::fs::create_dir_all(root.join("lua")).await.unwrap();
  for file in ["plugin.toml", "main.lua", "lua/greeting.lua"] {
    tokio::fs::copy(
      Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/lua")
        .join(file),
      root.join(file),
    )
    .await
    .unwrap();
  }
  tokio::fs::write(
    temp.path().join("config.toml"),
    "api_key_env = \"\"\n[memory]\nbackend = \"off\"\n",
  )
  .await
  .unwrap();
  let approved = cli(temp.path())
    .args(["plugins", "approve", "greeting"])
    .output()
    .await
    .unwrap();
  assert!(
    approved.status.success(),
    "{}",
    String::from_utf8_lossy(&approved.stderr)
  );
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let endpoint = format!("http://{}", listener.local_addr().unwrap());
  let server = tokio::spawn(async move {
    for turn in 0..2 {
      let (mut socket, _) = listener.accept().await.unwrap();
      let request = request(&mut socket).await;
      let message = if turn == 0 {
        assert!(
          request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "greeting_hello")
        );
        json!({"content":null,"tool_calls":[{"id":"greet", "function":{"name":"greeting_hello","arguments":"{\"name\":\"Wess\"}"}}]})
      } else {
        let last = request["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "tool");
        assert_eq!(last["tool_call_id"], "greet");
        assert_eq!(
          serde_json::from_str::<Value>(last["content"].as_str().unwrap()).unwrap(),
          json!({"message":"hello, Wess"})
        );
        json!({"content":"greeting completed", "tool_calls":[]})
      };
      let body = json!({"choices":[{"message":message}]}).to_string();
      socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    }
  });
  let output = tokio::time::timeout(
    Duration::from_secs(10),
    cli(temp.path())
      .args([
        "--endpoint",
        &endpoint,
        "--model",
        "test",
        "--permissions",
        "read-only",
        "ask",
        "--json",
        "--no-save",
        "greet Wess",
      ])
      .output(),
  )
  .await
  .unwrap()
  .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("greeting completed"));
  assert!(stdout.contains("tool_end"));
  server.await.unwrap();
}
