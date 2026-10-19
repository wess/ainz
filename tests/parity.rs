use std::{collections::BTreeMap, sync::Arc};

use ainz::{
  ChatProvider, Config, Event, EventSink, HttpProvider, ProviderConfig, RuntimeProvider, Session,
  SessionStore, WireApi,
  protocol::{Message, Role, ToolSpec},
};
use serde_json::{Value, json};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
  sync::Mutex,
};

type Requests = Arc<Mutex<Vec<(String, Value)>>>;

async fn server(replies: Vec<(&str, String)>) -> (String, Requests, tokio::task::JoinHandle<()>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let endpoint = format!("http://{}", listener.local_addr().unwrap());
  let requests = Arc::new(Mutex::new(Vec::new()));
  let seen = requests.clone();
  let replies: Vec<_> = replies
    .into_iter()
    .map(|(kind, body)| (kind.to_string(), body))
    .collect();
  let task = tokio::spawn(async move {
    for (kind, body) in replies {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut bytes = Vec::new();
      let split = loop {
        let mut chunk = [0; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
          break index + 4;
        }
      };
      let head = String::from_utf8(bytes[..split].to_vec()).unwrap();
      let length = head
        .lines()
        .find_map(|line| {
          let (name, value) = line.split_once(':')?;
          name
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
      while bytes.len() - split < length {
        let mut chunk = [0; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
      }
      seen.lock().await.push((
        head,
        serde_json::from_slice(&bytes[split..split + length]).unwrap(),
      ));
      let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
      );
      socket.write_all(reply.as_bytes()).await.unwrap();
    }
  });
  (endpoint, requests, task)
}

fn sse(events: &[Value]) -> String {
  events
    .iter()
    .map(|event| format!("data: {event}\n\n"))
    .collect()
}

fn response(text: &str) -> Value {
  json!({"status":"completed", "output":[{"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":text}]}], "usage":{"input_tokens":12,"output_tokens":3}})
}

#[tokio::test]
async fn responses_preserves_encrypted_reasoning_across_tools_and_saved_sessions() {
  let output = json!([
    {"type":"reasoning","id":"r1","encrypted_content":"sealed","summary":[]},
    {"type":"function_call","id":"fc1","call_id":"call1","name":"read","arguments":"{\"path\":\"Cargo.toml\"}","status":"completed"}
  ]);
  let first =
    json!({"status":"completed","output":output,"usage":{"input_tokens":20,"output_tokens":4}});
  let (endpoint, requests, host) = server(vec![
    (
      "text/event-stream",
      sse(&[json!({"type":"response.completed","response":first})]),
    ),
    ("application/json", response("done").to_string()),
  ])
  .await;
  let profile = ProviderConfig {
    wire_api: WireApi::Responses,
    reasoning_effort: Some("high".into()),
    ..ProviderConfig::http(endpoint, "")
  };
  let config = Config::default();
  let workspace = tempfile::tempdir().unwrap();
  let provider = RuntimeProvider::from_profile(&config, &profile, "test".into(), workspace.path())
    .await
    .unwrap();
  let user = Message::text(Role::User, "read the manifest");
  let tools = [ToolSpec {
    name: "read".into(),
    description: "Read a file".into(),
    parameters: json!({"type":"object"}),
  }];
  let reply = provider
    .complete(&[&user], &tools, &EventSink::default())
    .await
    .unwrap();
  assert_eq!(reply.message.tool_calls[0].id, "call1");
  let mut session = Session::new(workspace.path().into());
  session.append(user);
  session.append(reply.message);
  session.append(Message::tool("call1", "manifest"));
  let store = SessionStore::new(workspace.path().join("sessions"));
  store.save(&session).await.unwrap();
  let loaded = store.load(session.id).await.unwrap();
  let messages = loaded.messages().unwrap();
  assert_eq!(json!(messages[1].response_items), output);
  let answer = provider
    .complete(
      &messages.iter().collect::<Vec<_>>(),
      &tools,
      &EventSink::default(),
    )
    .await
    .unwrap();
  assert_eq!(answer.message.content.as_deref(), Some("done"));
  host.await.unwrap();
  let requests = requests.lock().await;
  assert!(requests[0].0.starts_with("POST /responses "));
  assert_eq!(requests[0].1["store"], false);
  assert_eq!(requests[0].1["reasoning"]["effort"], "high");
  assert_eq!(requests[1].1["input"][1]["encrypted_content"], "sealed");
  assert_eq!(requests[1].1["input"][3]["call_id"], "call1");
}

#[tokio::test]
async fn messages_replays_signed_thinking_and_tool_results_with_native_authentication() {
  let body = sse(&[
    json!({"type":"message_start","message":{"usage":{"input_tokens":12}}}),
    json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"consider"}}),
    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"nature"}}),
    json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"list"}}),
    json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"tool_use"}}),
    json!({"type":"message_stop"}),
  ]);
  let (endpoint, requests, host) = server(vec![
    ("text/event-stream", body.clone()),
    ("text/event-stream", body),
  ])
  .await;
  let mut profile = ProviderConfig::anthropic();
  profile.endpoint = Some(endpoint);
  profile.api_key_env.clear();
  profile
    .headers
    .insert("x-api-key".into(), "test-key".into());
  profile.thinking_tokens = 1024;
  let provider = RuntimeProvider::from_profile(
    &Config::default(),
    &profile,
    "test".into(),
    std::path::Path::new("."),
  )
  .await
  .unwrap();
  let user = Message::text(Role::User, "list files");
  let reply = provider
    .complete(
      &[&Message::text(Role::System, "instructions"), &user],
      &[],
      &EventSink::default(),
    )
    .await
    .unwrap();
  assert_eq!(
    reply
      .message
      .thinking
      .as_ref()
      .unwrap()
      .signature
      .as_deref(),
    Some("signature")
  );
  assert_eq!(reply.usage.input_tokens, 12);
  let tool = Message::tool("t1", "files");
  provider
    .complete(&[&user, &reply.message, &tool], &[], &EventSink::default())
    .await
    .unwrap();
  host.await.unwrap();
  let requests = requests.lock().await;
  assert!(requests[0].0.starts_with("POST /messages "));
  assert!(requests[0].0.contains("x-api-key: test-key"));
  assert_eq!(requests[0].1["system"], "instructions");
  assert_eq!(
    requests[1].1["messages"][1]["content"][0]["signature"],
    "signature"
  );
  assert_eq!(
    requests[1].1["messages"][2]["content"][0]["tool_use_id"],
    "t1"
  );
}

#[tokio::test]
async fn a_failed_partial_reply_never_leaks_into_a_successful_fallback() {
  let bad = sse(&[json!({"choices":[{"delta":{"content":"discard me"}}]})]);
  let good =
    json!({"choices":[{"message":{"content":"backup answer"},"finish_reason":"stop"}]}).to_string();
  let (first, _, firsthost) = server(vec![("text/event-stream", bad)]).await;
  let (second, _, secondhost) = server(vec![("application/json", good)]).await;
  let provider = RuntimeProvider::Fallback(vec![
    (
      "first".into(),
      Box::new(RuntimeProvider::Http(
        HttpProvider::new(first, "test".into(), None, 0).unwrap(),
      )),
    ),
    (
      "backup".into(),
      Box::new(RuntimeProvider::Http(
        HttpProvider::new(second, "test".into(), None, 0).unwrap(),
      )),
    ),
  ]);
  let (sink, mut events) = EventSink::channel();
  let reply = provider
    .complete(&[&Message::text(Role::User, "hello")], &[], &sink)
    .await
    .unwrap();
  assert_eq!(reply.message.content.as_deref(), Some("backup answer"));
  let mut text = String::new();
  while let Ok(event) = events.try_recv() {
    if let Event::TextDelta { text: part } = event {
      text.push_str(&part);
    }
  }
  assert_eq!(text, "backup answer");
  firsthost.await.unwrap();
  secondhost.await.unwrap();
}

#[tokio::test]
async fn incomplete_native_responses_fail_instead_of_becoming_final_answers() {
  let (endpoint, _, host) = server(vec![(
    "application/json",
    json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}).to_string(),
  )])
  .await;
  let profile = ProviderConfig {
    wire_api: WireApi::Responses,
    ..ProviderConfig::http(endpoint, "")
  };
  let provider = RuntimeProvider::from_profile(
    &Config::default(),
    &profile,
    "test".into(),
    std::path::Path::new("."),
  )
  .await
  .unwrap();
  let error = provider
    .complete(
      &[&Message::text(Role::User, "hello")],
      &[],
      &EventSink::default(),
    )
    .await
    .unwrap_err();
  assert!(error.to_string().contains("max_output_tokens"));
  host.await.unwrap();
}

#[tokio::test]
async fn checkpoints_restore_the_snapshot_without_destroying_the_current_conversation() {
  let root = tempfile::tempdir().unwrap();
  let store = SessionStore::new(root.path().into());
  let mut session = Session::new(root.path().into());
  session.append(Message::text(Role::User, "before"));
  store.save(&session).await.unwrap();
  let checkpoint = store.checkpoint(&session, "before").await.unwrap();
  session.append(Message::text(Role::Assistant, "after"));
  store.save(&session).await.unwrap();
  let restored = store.restore(session.id, "before").await.unwrap();
  assert_eq!(restored.messages().unwrap().len(), 1);
  assert_eq!(
    store
      .load(session.id)
      .await
      .unwrap()
      .messages()
      .unwrap()
      .len(),
    2
  );
  assert_eq!(store.checkpoints(session.id).await.unwrap(), ["before"]);
  assert!(store.checkpoint(&session, "../escape").await.is_err());
  assert!(checkpoint.exists());
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
      std::fs::metadata(checkpoint).unwrap().permissions().mode() & 0o777,
      0o600
    );
  }
}

#[tokio::test]
async fn fallback_configuration_uses_the_backup_profiles_model() {
  let good = json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}).to_string();
  let (endpoint, requests, host) = server(vec![("application/json", good)]).await;
  let config = Config {
    model: "active-model".into(),
    provider: Some("primary".into()),
    provider_retries: 0,
    fallback_providers: vec!["backup".into()],
    providers: BTreeMap::from([
      (
        "primary".into(),
        ProviderConfig::http("http://127.0.0.1:1", ""),
      ),
      (
        "backup".into(),
        ProviderConfig {
          model: Some("backup-model".into()),
          ..ProviderConfig::http(endpoint, "")
        },
      ),
    ]),
    ..Config::default()
  };
  let provider = RuntimeProvider::configured(&config, std::path::Path::new("."))
    .await
    .unwrap();
  provider
    .complete(
      &[&Message::text(Role::User, "hello")],
      &[],
      &EventSink::default(),
    )
    .await
    .unwrap();
  host.await.unwrap();
  assert_eq!(requests.lock().await[0].1["model"], "backup-model");
}

#[tokio::test]
async fn named_agent_types_use_the_nearest_definition_and_enforce_tool_filters() {
  let root = tempfile::tempdir().unwrap();
  let workspace = root.path().join("project");
  tokio::fs::create_dir_all(workspace.join(".ainz/agents"))
    .await
    .unwrap();
  tokio::fs::create_dir_all(root.path().join(".claude/agents"))
    .await
    .unwrap();
  tokio::fs::write(
    root.path().join(".claude/agents/parityreviewer.md"),
    "---\nname: parityreviewer\n---\nouter",
  )
  .await
  .unwrap();
  tokio::fs::write(workspace.join(".ainz/agents/parityreviewer.md"), "---\nname: parityreviewer\ntools: read, shell\ndisallowedTools: shell\npermissionMode: plan\nmaxTurns: 3\nmodel: review-model\n---\nReview carefully.").await.unwrap();
  let catalog = ainz::subagent::AgentCatalog::discover(&workspace)
    .await
    .unwrap();
  let definition = catalog.get("parityreviewer").unwrap();
  assert_eq!(definition.instructions, "Review carefully.");
  assert_eq!(definition.max_steps, Some(3));
  assert_eq!(
    definition.permission_mode,
    Some(ainz::PermissionMode::ReadOnly)
  );
  let mut tools = ainz::tool::ToolSet::default();
  tools.extend(ainz::tool::builtins()).unwrap();
  let restricted = tools.restricted(
    definition.tools.as_deref(),
    definition.disallowed_tools.as_deref(),
  );
  assert!(restricted.get("read").is_some());
  assert!(restricted.get("shell").is_none());
  assert!(restricted.get("write").is_none());
}

#[tokio::test]
async fn a_reported_cost_budget_stops_before_requested_tools_execute() {
  use ainz::{
    Agent, PermissionMode, RunOptions,
    tool::{ToolSet, builtins},
  };
  let workspace = tempfile::tempdir().unwrap();
  let body = json!({"choices":[{"message":{"tool_calls":[{"id":"w1","function":{"name":"write","arguments":"{\"path\":\"blocked\",\"content\":\"unexpected\"}"}}]},"finish_reason":"tool_calls"}], "usage":{"prompt_tokens":1,"completion_tokens":1,"cost_usd":0.5}}).to_string();
  let (endpoint, _, host) = server(vec![("application/json", body)]).await;
  let provider = HttpProvider::new(endpoint, "test".into(), None, 0).unwrap();
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let agent = Agent::new(
    provider,
    tools,
    workspace.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(workspace.path().into());
  let options = RunOptions {
    budget_usd: Some(0.1),
    permissions: PermissionMode::Auto,
    ..RunOptions::default()
  };
  let error = agent
    .run(&mut session, "write".into(), options)
    .await
    .unwrap_err();
  assert!(error.to_string().contains("run budget"));
  assert!(!workspace.path().join("blocked").exists());
  assert_eq!(session.usage.cost_usd, Some(0.5));
  assert_eq!(session.messages().unwrap().last().unwrap().role, Role::Tool);
  host.await.unwrap();
}

#[cfg(feature = "cli")]
#[tokio::test]
async fn shell_commands_save_native_profiles_fallbacks_and_remote_transports() {
  let root = tempfile::tempdir().unwrap();
  let configpath = root.path().join("config.toml");
  let mcppath = root.path().join("mcp.toml");
  for args in [
    vec!["providers", "add", "primary", "--preset", "openai"],
    vec![
      "providers",
      "add",
      "backup",
      "--preset",
      "anthropic",
      "--model",
      "backup-model",
    ],
    vec!["providers", "use", "primary", "primary-model"],
    vec!["providers", "fallback", "backup"],
    vec!["mcp", "add-sse", "remote", "https://tools.example.org/sse"],
  ] {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_ainz"))
      .args(args)
      .env("AINZ_CONFIG", &configpath)
      .env("AINZ_MCP_PROFILE", &mcppath)
      .env_remove("AINZ_MODEL")
      .env_remove("AINZ_PROVIDER")
      .env_remove("AINZ_ENDPOINT")
      .env_remove("AINZ_FALLBACK_PROVIDERS")
      .output()
      .await
      .unwrap();
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
  }
  let config: Config =
    toml::from_str(&tokio::fs::read_to_string(configpath).await.unwrap()).unwrap();
  assert_eq!(config.providers["primary"].wire_api, WireApi::Responses);
  assert_eq!(config.providers["backup"].wire_api, WireApi::Messages);
  assert_eq!(
    config.providers["backup"].model.as_deref(),
    Some("backup-model")
  );
  assert_eq!(config.fallback_providers, ["backup"]);
  let profile: ainz::McpProfile =
    toml::from_str(&tokio::fs::read_to_string(mcppath).await.unwrap()).unwrap();
  assert_eq!(profile.servers["remote"].transport, ainz::McpTransport::Sse);
}

#[cfg(feature = "cli")]
#[tokio::test]
async fn print_mode_sends_piped_input_with_the_request_and_emits_json_events() {
  use std::process::Stdio;
  let root = tempfile::tempdir().unwrap();
  let (endpoint, requests, host) = server(vec![(
    "application/json",
    json!({"choices":[{"message":{"content":"diagnosis"},"finish_reason":"stop"}]}).to_string(),
  )])
  .await;
  let config = Config {
    model: "test".into(),
    endpoint,
    api_key_env: String::new(),
    memory: ainz::MemoryConfig {
      backend: ainz::MemoryBackend::Off,
      ..Default::default()
    },
    ..Config::default()
  };
  let configpath = root.path().join("config.toml");
  config.save_to(&configpath).await.unwrap();
  let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ainz"))
    .args([
      "--workspace",
      root.path().to_str().unwrap(),
      "-p",
      "explain",
      "--output-format",
      "json",
    ])
    .env("HOME", root.path())
    .env("AINZ_CONFIG", configpath)
    .env("AINZ_MCP_PROFILE", root.path().join("mcp.toml"))
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  child
    .stdin
    .take()
    .unwrap()
    .write_all(b"build failed: missing symbol")
    .await
    .unwrap();
  let output = child.wait_with_output().await.unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let events: Vec<Value> = String::from_utf8(output.stdout)
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect();
  assert!(events.iter().any(|event| event["type"] == "turn_end"));
  host.await.unwrap();
  let requests = requests.lock().await;
  let prompt = requests[0].1["messages"]
    .as_array()
    .unwrap()
    .last()
    .unwrap()["content"]
    .as_str()
    .unwrap();
  assert!(prompt.contains("explain"));
  assert!(prompt.contains("missing symbol"));
}
