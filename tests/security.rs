use ainz::{
  Agent, ChatProvider, EventSink, HookRunner, HttpProvider, PermissionMode, PermissionRules,
  ProcessOutput, ProcessProvider, RunOptions, Session,
  protocol::{Message, Role, ToolCall, ToolSpec, Usage},
  provider::ProviderReply,
  tool::{ToolContext, ToolSet, builtins},
};
use async_trait::async_trait;
use serde_json::json;
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
};

fn options() -> RunOptions {
  RunOptions {
    instructions: "test".into(),
    permissions: PermissionMode::Auto,
    rules: PermissionRules::default(),
    max_steps: 2,
    max_output_bytes: 1024,
    context_tokens: 16000,
    compact_at_tokens: 12000,
    preserve_messages: 4,
    memory_nudge: None,
    hooks: HookRunner::default(),
  }
}
struct Script(Mutex<VecDeque<ProviderReply>>);
#[async_trait]
impl ChatProvider for Script {
  async fn complete(
    &self,
    messages: &[Message],
    _: &[ToolSpec],
    _: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    let mut pending = std::collections::BTreeSet::new();
    for message in messages {
      if message.role == Role::Tool {
        assert!(pending.remove(message.tool_call_id.as_deref().unwrap()));
      } else {
        assert!(
          pending.is_empty(),
          "conversation contains unanswered tool calls"
        );
        pending.extend(message.tool_calls.iter().map(|call| call.id.as_str()));
      }
    }
    assert!(
      pending.is_empty(),
      "conversation ends with unanswered tool calls"
    );
    Ok(self.0.lock().unwrap().pop_front().unwrap())
  }
}
fn script(calls: Vec<ToolCall>) -> Script {
  Script(Mutex::new(VecDeque::from([
    ProviderReply {
      message: Message {
        role: Role::Assistant,
        content: None,
        tool_calls: calls,
        tool_call_id: None,
        images: vec![],
      },
      usage: Usage::default(),
    },
    ProviderReply {
      message: Message::text(Role::Assistant, "done"),
      usage: Usage::default(),
    },
  ])))
}
fn tools() -> ToolSet {
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  tools
}

#[tokio::test]
async fn dangling_symlink_cannot_write_outside_workspace() {
  let root = tempfile::tempdir().unwrap();
  let outside = tempfile::tempdir().unwrap();
  let target = outside.path().join("created");
  std::os::unix::fs::symlink(&target, root.path().join("link")).unwrap();
  tools()
    .get("write")
    .unwrap()
    .execute(
      &ToolContext::new(root.path().into(), uuid::Uuid::nil(), 1024),
      json!({"path":"link", "content":"escaped"}),
    )
    .await
    .unwrap_err();
  assert!(!target.exists());
}

#[tokio::test]
async fn denied_paths_reject_aliases_and_decoy_arguments() {
  for args in [
    json!({"path":"./protected", "content":"changed"}),
    json!({"path":"protected", "content":"changed", "command":"decoy"}),
  ] {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("protected"), "original").unwrap();
    let agent = Agent::new(
      script(vec![ToolCall {
        id: "1".into(),
        name: "write".into(),
        arguments: args,
      }]),
      tools(),
      root.path().into(),
      EventSink::default(),
      ainz::deny_all(),
    );
    let mut opts = options();
    opts.rules.deny.push("write(protected)".into());
    agent
      .run(&mut Session::new(root.path().into()), "test".into(), opts)
      .await
      .unwrap();
    assert_eq!(
      std::fs::read_to_string(root.path().join("protected")).unwrap(),
      "original"
    );
  }
}

#[tokio::test]
async fn shell_allowance_does_not_authorize_a_second_command() {
  let root = tempfile::tempdir().unwrap();
  let agent = Agent::new(
    script(vec![ToolCall {
      id: "1".into(),
      name: "shell".into(),
      arguments: json!({"command":"git --version; printf escaped > marker"}),
    }]),
    tools(),
    root.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut opts = options();
  opts.permissions = PermissionMode::ReadOnly;
  opts.rules.allow.push("shell(git *)".into());
  agent
    .run(&mut Session::new(root.path().into()), "test".into(), opts)
    .await
    .unwrap();
  assert!(!root.path().join("marker").exists());
}

#[tokio::test]
async fn mapped_loopback_fetch_cannot_reach_local_service() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut buf = [0; 8192];
    let _ = socket.read(&mut buf).await.unwrap();
    socket
      .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nlocal")
      .await
      .unwrap();
  });
  let root = tempfile::tempdir().unwrap();
  let output = tokio::time::timeout(
    Duration::from_secs(3),
    tools().get("fetch").unwrap().execute(
      &ToolContext::new(root.path().into(), uuid::Uuid::nil(), 1024),
      json!({"url":format!("http://[::ffff:127.0.0.1]:{port}/")}),
    ),
  )
  .await
  .unwrap()
  .unwrap_err();
  assert!(output.to_string().contains("not a public address"));
  server.abort();
}

#[tokio::test]
async fn cancellation_closes_every_outstanding_tool_call() {
  let root = tempfile::tempdir().unwrap();
  let (controller, mut inbox) = ainz::run_control();
  let cancel = controller.clone();
  let events = EventSink::new(move |event| {
    if matches!(event, ainz::Event::ToolStart { .. }) {
      cancel.cancel();
    }
  });
  let agent = Agent::new(
    script(vec![
      ToolCall {
        id: "1".into(),
        name: "shell".into(),
        arguments: json!({"command":"sleep 5"}),
      },
      ToolCall {
        id: "2".into(),
        name: "read".into(),
        arguments: json!({"path":"missing"}),
      },
    ]),
    tools(),
    root.path().into(),
    events,
    ainz::deny_all(),
  );
  let mut session = Session::new(root.path().into());
  assert!(
    agent
      .run_controlled(&mut session, "test".into(), options(), &mut inbox)
      .await
      .is_err()
  );
  let messages = session.context_messages().unwrap();
  let results: Vec<_> = messages.iter().filter(|m| m.role == Role::Tool).collect();
  assert_eq!(results.len(), 2);
  assert_eq!(results[0].tool_call_id.as_deref(), Some("1"));
  assert_eq!(results[1].tool_call_id.as_deref(), Some("2"));
  agent
    .run(&mut session, "resume".into(), options())
    .await
    .unwrap();
}

#[tokio::test]
async fn cancelling_process_provider_terminates_descendants() {
  let root = tempfile::tempdir().unwrap();
  let provider = ProcessProvider::new(
    "/bin/sh".into(),
    vec![
      "-c".into(),
      "(sleep 0.3; printf survived > marker) & wait".into(),
    ],
    "test".into(),
    root.path().into(),
    PermissionMode::Auto,
    ProcessOutput::Text,
  );
  assert!(
    tokio::time::timeout(
      Duration::from_millis(100),
      provider.complete(&[], &[], &EventSink::default())
    )
    .await
    .is_err()
  );
  tokio::time::sleep(Duration::from_millis(500)).await;
  assert!(!root.path().join("marker").exists());
}

#[tokio::test]
async fn incomplete_provider_stream_is_rejected() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut buf = [0; 8192];
    let _ = socket.read(&mut buf).await.unwrap();
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"unfinished\"}}]}\n\n";
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
  });
  let provider = HttpProvider::new(format!("http://{address}"), "test".into(), None, 0).unwrap();
  let error = provider
    .complete(&[], &[], &EventSink::default())
    .await
    .unwrap_err();
  assert!(error.to_string().contains("before completion"));
  server.await.unwrap();
}

#[tokio::test]
async fn file_denials_match_both_rule_and_argument_aliases() {
  for rule in ["write(protected)", "write(./protected)", "write(alias)"] {
    for path in ["protected", "./protected", "alias"] {
      let root = tempfile::tempdir().unwrap();
      std::fs::write(root.path().join("protected"), "original").unwrap();
      std::os::unix::fs::symlink("protected", root.path().join("alias")).unwrap();
      let agent = Agent::new(
        script(vec![ToolCall {
          id: "1".into(),
          name: "write".into(),
          arguments: json!({"path":path,"content":"changed"}),
        }]),
        tools(),
        root.path().into(),
        EventSink::default(),
        ainz::deny_all(),
      );
      let mut opts = options();
      opts.rules.deny.push(rule.into());
      agent
        .run(&mut Session::new(root.path().into()), "test".into(), opts)
        .await
        .unwrap();
      assert_eq!(
        std::fs::read_to_string(root.path().join("protected")).unwrap(),
        "original",
        "{rule}: {path}"
      );
    }
  }
}
