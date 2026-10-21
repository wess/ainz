use std::path::Path;

use serde_json::{Value, json};
use tokio::{
  fs,
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  net::TcpListener,
  process::Command,
};

pub const ORIGINAL: &str = "pub use runtime::*;\npub const STATUS: &str = \"broken\";\n";
pub const REPAIRED: &str = "pub use runtime::*;\npub const STATUS: &str = \"fixed\";\n";

pub async fn repository(root: &Path) {
  fs::create_dir_all(root.join("src")).await.unwrap();
  let manifest = json!({"package":{"name":"ainz", "version":"0.0.0", "edition":"2024"},
    "dependencies":{"runtime":{"package":"ainz", "path":env!("CARGO_MANIFEST_DIR"), "default-features":false}, "anyhow":"1"}});
  fs::write(root.join("Cargo.toml"), toml::to_string(&manifest).unwrap())
    .await
    .unwrap();
  fs::write(root.join("src/lib.rs"), ORIGINAL).await.unwrap();
  fs::write(root.join(".gitignore"), "target/\n")
    .await
    .unwrap();
  for args in [vec!["init", "-q"], vec!["add", "."]] {
    assert!(
      Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .await
        .unwrap()
        .success()
    );
  }
}

pub async fn provider(drift: bool) -> (String, tokio::task::JoinHandle<()>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let task = tokio::spawn(async move {
    loop {
      let (socket, _) = listener.accept().await.unwrap();
      let mut socket = BufReader::new(socket);
      let mut length = 0;
      loop {
        let mut line = String::new();
        if socket.read_line(&mut line).await.unwrap() == 0 {
          return;
        }
        if line == "\r\n" {
          break;
        }
        if let Some((key, value)) = line.split_once(':')
          && key.eq_ignore_ascii_case("content-length")
        {
          length = value.trim().parse::<usize>().unwrap();
        }
      }
      let mut bytes = vec![0; length];
      socket.read_exact(&mut bytes).await.unwrap();
      let request: Value = serde_json::from_slice(&bytes).unwrap();
      let message = respond(&request, drift);
      let body =
        json!({"choices":[{"message":message}], "usage":{"prompt_tokens":3,"completion_tokens":1}})
          .to_string();
      let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
      );
      if socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .is_err()
      {
        continue;
      }
    }
  });
  (format!("http://{address}"), task)
}

fn respond(request: &Value, drift: bool) -> Value {
  let messages = request["messages"].as_array().unwrap();
  if messages.last().unwrap()["role"] == "tool" {
    return json!({"role":"assistant", "content":"done", "tool_calls":[]});
  }
  let prompt = messages.iter().rev().find(|m| m["role"] == "user").unwrap()["content"]
    .as_str()
    .unwrap();
  let (path, content) = if prompt.starts_with("Repair total") {
    (
      "src/lib.rs",
      "pub fn total(values: &[i64]) -> i64 { values.iter().copied().filter(|n| *n > 0).sum() }\n",
    )
  } else if prompt.starts_with("Repair normalize") {
    (
      "src/lib.rs",
      "pub fn normalize(value: &str) -> String { value.trim().to_ascii_lowercase() }\n",
    )
  } else if prompt.starts_with("Read request.json") {
    let loaded = messages.iter().any(|m| m["role"] == "tool");
    (
      "decision.json",
      if drift && loaded {
        "{\"status\":\"ready\",\"missing\":[]}"
      } else {
        "{\"status\":\"blocked\",\"missing\":[\"deploy_target\"]}"
      },
    )
  } else if prompt.starts_with("Improve the Ainz harness") {
    return json!({"role":"assistant", "content":null, "tool_calls":[
      call("write", json!({"path":"src/lib.rs", "content":REPAIRED})),
      call("learn", json!({"action":"teach", "name":"verify-fix", "description":"Verify repairs", "instructions":"Reproduce, repair, then verify."}))
    ]});
  } else {
    return json!({"role":"assistant", "content":"ordinary observed failure", "tool_calls":[]});
  };
  json!({"role":"assistant", "content":null, "tool_calls":[call("write", json!({"path":path,"content":content}))]})
}

fn call(name: &str, args: Value) -> Value {
  json!({"id":name,"type":"function", "function":{"name":name,"arguments":args.to_string()}})
}
