#![cfg(feature = "cli")]

use std::{
  path::Path,
  process::Output,
  sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  net::TcpListener,
  process::Command,
  task::JoinHandle,
};

struct Fixture {
  endpoint: String,
  requests: Arc<Mutex<Vec<(String, Value)>>>,
  task: JoinHandle<()>,
}

impl Fixture {
  async fn start(verdict: &'static str) -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let task = tokio::spawn(async move {
      while let Ok((socket, _)) = listener.accept().await {
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        let path = line.split_whitespace().nth(1).unwrap().to_string();
        let mut length = 0;
        loop {
          line.clear();
          socket.read_line(&mut line).await.unwrap();
          if line == "\r\n" {
            break;
          }
          if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
          }
        }
        let mut bytes = vec![0; length];
        socket.read_exact(&mut bytes).await.unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        let messages = request["messages"].as_array().unwrap();
        let tool = messages.iter().any(|message| message["role"] == "tool");
        let content = if path.starts_with("/boss/") {
          json!({"content": verdict})
        } else if path.starts_with("/cheap/")
          && !tool
          && bytes.windows(13).any(|w| w == b"read the file")
        {
          json!({"tool_calls": [{
            "id": "lookup",
            "function": {"name": "read", "arguments": "{\"path\":\"example.txt\"}"}
          }]})
        } else if path.starts_with("/cheap/")
          && !tool
          && bytes.windows(14).any(|w| w == b"delegate a job")
        {
          json!({"tool_calls": [{
            "id": "delegate",
            "function": {"name": "subagent", "arguments": "{\"role\":\"reviewer\",\"prompt\":\"check this\"}"}
          }]})
        } else {
          json!({"content": format!("answered by {path}")})
        };
        seen.lock().unwrap().push((path, request));
        let reply = json!({
          "choices": [{"message": content}],
          "usage": {"prompt_tokens": 3, "completion_tokens": 2, "cost_usd": 0.02}
        })
        .to_string();
        let response = format!(
          "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
          reply.len(),
        );
        socket
          .get_mut()
          .write_all(response.as_bytes())
          .await
          .unwrap();
      }
    });
    Self {
      endpoint,
      requests,
      task,
    }
  }

  fn take(&self) -> Vec<(String, Value)> {
    std::mem::take(&mut *self.requests.lock().unwrap())
  }
}

impl Drop for Fixture {
  fn drop(&mut self) {
    self.task.abort();
  }
}

fn configure(root: &Path, endpoint: &str, strategy: &str, permissions: &str) {
  std::fs::write(
    root.join("config.toml"),
    format!(
      r#"
provider = "cheap"
model = "shared"
permissions = "{permissions}"
provider_retries = 0

[boss]
enabled = true
strategy = "{strategy}"
router = {{ provider = "boss", model = "route" }}
candidates = [
  {{ provider = "cheap", model = "shared", cost = "low", quality = "basic" }},
  {{ provider = "medium", model = "middle", cost = "medium", quality = "capable" }},
  {{ provider = "strong", model = "shared", cost = "high", quality = "strong", images = true }},
]

[providers.boss]
kind = "http"
endpoint = "{endpoint}/boss"

[providers.cheap]
kind = "http"
endpoint = "{endpoint}/cheap"
models = ["shared"]

[providers.medium]
kind = "http"
endpoint = "{endpoint}/medium"
model = "middle"

[providers.medium.model_hints]
middle = "good at implementation"

[providers.strong]
kind = "http"
endpoint = "{endpoint}/strong"
models = ["shared"]

[memory]
backend = "off"

[update]
auto = false
"#
    ),
  )
  .unwrap();
}

async fn ask(root: &Path, flags: &[&str], ask: &[&str], prompt: &str) -> Output {
  let mut command = Command::new(env!("CARGO_BIN_EXE_ainz"));
  command
    .current_dir(root)
    .env("HOME", root)
    .env("XDG_CONFIG_HOME", root)
    .env("AINZ_CONFIG", root.join("config.toml"))
    .env_remove("AINZ_PROVIDER")
    .env_remove("AINZ_MODEL")
    .env_remove("AINZ_ENDPOINT")
    .env_remove("AINZ_SYNAPSE")
    .env_remove("AINZ_MEMORY")
    .arg("--clean")
    .args(flags)
    .args(["ask", "--json", "--no-save"])
    .args(ask)
    .arg(prompt);
  tokio::time::timeout(
    std::time::Duration::from_secs(15),
    command.kill_on_drop(true).output(),
  )
  .await
  .unwrap()
  .unwrap()
}

fn events(output: &Output) -> Vec<Value> {
  String::from_utf8_lossy(&output.stdout)
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect()
}

#[tokio::test]
async fn strategies_select_distinct_models_and_keep_the_worker_through_tools() {
  let server =
    Fixture::start(r#"{"required":"basic","preferred":2,"reason":"fit for the task"}"#).await;
  let root = tempfile::tempdir().unwrap();
  std::fs::write(root.path().join("example.txt"), "the requested file").unwrap();
  for (strategy, worker, model, calls) in [
    ("economy", "cheap", "shared", 3),
    ("balanced", "medium", "middle", 2),
    ("quality", "strong", "shared", 2),
  ] {
    configure(root.path(), &server.endpoint, strategy, "read_only");
    let output = ask(root.path(), &[], &[], "read the file").await;
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
    let events = events(&output);
    let selected = events
      .iter()
      .find(|event| event["type"] == "model_selected")
      .unwrap();
    assert_eq!(
      selected["target"],
      json!({"provider": worker, "model": model})
    );
    assert_eq!(selected["strategy"], strategy);
    let requests = server.take();
    assert_eq!(requests.len(), calls);
    assert_eq!(requests[0].1["model"], "route");
    assert!(requests[0].1.get("tools").is_none());
    let brief: Value =
      serde_json::from_str(requests[0].1["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(brief["task"], "read the file");
    assert_eq!(brief["candidates"][1]["hint"], "good at implementation");
    for (path, request) in &requests[1..] {
      assert!(path.starts_with(&format!("/{worker}/")), "{path}");
      assert_eq!(request["model"], model);
    }
    let completed = events
      .iter()
      .find(|event| event["type"] == "turn_end")
      .unwrap();
    assert_eq!(completed["usage"]["input_tokens"], 3 * calls);
    if calls == 3 {
      assert!(
        requests[2].1["messages"]
          .to_string()
          .contains("the requested file")
      );
    }
  }
}

#[tokio::test]
async fn a_single_image_worker_and_explicit_overrides_skip_the_boss() {
  let server = Fixture::start("unexpected router call").await;
  let root = tempfile::tempdir().unwrap();
  configure(root.path(), &server.endpoint, "economy", "read_only");
  std::fs::write(root.path().join("image.png"), [1, 2, 3]).unwrap();
  let output = ask(root.path(), &[], &["--image", "image.png"], "look at this").await;
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(
    events(&output)
      .iter()
      .any(|event| event["reason"] == "only eligible model")
  );
  let requests = server.take();
  assert_eq!(requests.len(), 1);
  assert!(requests[0].0.starts_with("/strong/"));
  assert!(
    requests[0].1["messages"]
      .to_string()
      .contains("data:image/png;base64,AQID")
  );

  let output = ask(
    root.path(),
    &["--provider", "medium", "--model", "middle"],
    &[],
    "hello",
  )
  .await;
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(
    events(&output)
      .iter()
      .all(|event| event["type"] != "model_selected")
  );
  let requests = server.take();
  assert_eq!(requests.len(), 1);
  assert!(requests[0].0.starts_with("/medium/"));

  let path = root.path().join("config.toml");
  let config = std::fs::read_to_string(&path)
    .unwrap()
    .replace("router = { provider = \"boss\", model = \"route\" }\n", "");
  std::fs::write(path, format!("{config}\n[providers.direct]\nkind = \"process\"\ncommand = \"/bin/echo\"\nargs = [\"manual process\"]\n")).unwrap();
  let output = ask(
    root.path(),
    &["--provider", "direct", "--model", "manual"],
    &[],
    "hello",
  )
  .await;
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(
    events(&output)
      .iter()
      .any(|event| event["text"] == "manual process")
  );
  assert!(server.take().is_empty());
}

#[tokio::test]
async fn malformed_router_output_falls_back_and_still_counts_toward_the_budget() {
  let server = Fixture::start("invalid decision").await;
  let root = tempfile::tempdir().unwrap();
  configure(root.path(), &server.endpoint, "quality", "read_only");
  let output = ask(root.path(), &[], &[], "hello").await;
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let events_seen = events(&output);
  assert!(
    events_seen
      .iter()
      .any(|event| event["target"]["provider"] == "cheap")
  );
  let completed = events_seen
    .iter()
    .find(|event| event["type"] == "turn_end")
    .unwrap();
  assert_eq!(completed["usage"]["cost_usd"], 0.04);
  assert_eq!(server.take().len(), 2);

  let output = ask(root.path(), &["--budget-usd", "0.01"], &[], "hello").await;
  assert!(!output.status.success());
  assert!(String::from_utf8_lossy(&output.stderr).contains("run budget"));
  let requests = server.take();
  assert_eq!(requests.len(), 1);
  assert!(requests[0].0.starts_with("/boss/"));
}

#[tokio::test]
async fn a_named_child_can_pin_a_different_provider() {
  let server = Fixture::start(r#"{"required":"basic","preferred":1,"reason":"routine"}"#).await;
  let root = tempfile::tempdir().unwrap();
  configure(root.path(), &server.endpoint, "economy", "auto");
  let agents = root.path().join(".ainz/agents");
  std::fs::create_dir_all(&agents).unwrap();
  std::fs::write(
    agents.join("reviewer.md"),
    "---\nname: reviewer\nprovider: strong\nmodel: shared\n---\nReview.",
  )
  .unwrap();
  let output = ask(root.path(), &[], &[], "delegate a job").await;
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let requests = server.take();
  let paths: Vec<_> = requests
    .iter()
    .map(|(path, _)| path.split('/').nth(1).unwrap())
    .collect();
  assert_eq!(paths, ["boss", "cheap", "strong", "cheap"]);
  assert!(
    requests[3].1["messages"]
      .to_string()
      .contains("answered by /strong")
  );
}
