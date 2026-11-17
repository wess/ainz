#![cfg(all(feature = "cli", feature = "lua"))]

//! init.lua reaching a real run: a tool it defines answers the model, and its `pre_tool`
//! handler refuses a call the permission mode would have let through.

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

const INIT: &str = r#"
print("init.lua loaded")
ainz.tool({
  name = "shout",
  description = "Says it louder",
  risk = "read",
  parameters = { type = "object", properties = { text = { type = "string" } } },
  run = function(args) return args.text:upper() end,
})
ainz.on("pre_tool", function(call)
  if call.name == "shell" and call.subject:find("rm ", 1, true) then
    return false, "refused by init.lua"
  end
end)
"#;

fn reply(message: Value) -> String {
  let body = json!({"choices":[{"message":message}]}).to_string();
  format!(
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
    body.len()
  )
}

fn last_tool(request: &Value) -> String {
  let last = request["messages"]
    .as_array()
    .unwrap()
    .last()
    .unwrap()
    .clone();
  assert_eq!(last["role"], "tool");
  last["content"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn init_lua_gives_the_model_a_tool_and_refuses_a_call() {
  let temp = tempfile::tempdir().unwrap();
  tokio::fs::write(
    temp.path().join("config.toml"),
    "api_key_env = \"\"\n[memory]\nbackend = \"off\"\n",
  )
  .await
  .unwrap();
  tokio::fs::write(temp.path().join("init.lua"), INIT)
    .await
    .unwrap();
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let endpoint = format!("http://{}", listener.local_addr().unwrap());
  let server = tokio::spawn(async move {
    for turn in 0..3 {
      let (mut socket, _) = listener.accept().await.unwrap();
      let request = request(&mut socket).await;
      let message = match turn {
        0 => {
          assert!(
            request["tools"]
              .as_array()
              .unwrap()
              .iter()
              .any(|tool| tool["function"]["name"] == "shout")
          );
          json!({"content":null,"tool_calls":[{"id":"a","function":{"name":"shout","arguments":"{\"text\":\"hi\"}"}}]})
        }
        1 => {
          assert_eq!(last_tool(&request), "HI");
          json!({"content":null,"tool_calls":[{"id":"b","function":{"name":"shell","arguments":"{\"command\":\"rm -rf build\"}"}}]})
        }
        _ => {
          let refused = last_tool(&request);
          assert!(refused.contains("refused by init.lua"), "{refused}");
          json!({"content":"done", "tool_calls":[]})
        }
      };
      socket.write_all(reply(message).as_bytes()).await.unwrap();
    }
  });
  let output = tokio::time::timeout(
    Duration::from_secs(20),
    cli(temp.path())
      .args([
        "--endpoint",
        &endpoint,
        "--model",
        "test",
        "--permissions",
        "auto",
        "ask",
        "--json",
        "--no-save",
        "go",
      ])
      .output(),
  )
  .await
  .unwrap()
  .unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(output.status.success(), "{stderr}");
  assert!(stderr.contains("init.lua loaded"), "{stderr}");
  assert!(String::from_utf8(output.stdout).unwrap().contains("done"));
  assert!(!temp.path().join("build").exists());
  server.await.unwrap();
}

#[tokio::test]
async fn clean_runs_without_init_lua() {
  let temp = tempfile::tempdir().unwrap();
  tokio::fs::write(temp.path().join("config.toml"), "model = \"m\"\n[memory]\nbackend = \"off\"\n[providers.echo]\nkind = \"process\"\ncommand = \"/bin/echo\"\nargs = [\"echoed\"]\n")
    .await
    .unwrap();
  tokio::fs::write(temp.path().join("init.lua"), "print('should not run')")
    .await
    .unwrap();
  let output = cli(temp.path())
    .args(["--clean", "--provider", "echo", "-p", "hi"])
    .stdin(std::process::Stdio::null())
    .output()
    .await
    .unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(output.status.success(), "{stderr}");
  assert!(!stderr.contains("should not run"), "{stderr}");
}
