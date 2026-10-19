use std::{collections::VecDeque, sync::Mutex};

use ainz::{
  Agent, EventSink, HookRunner, PermissionMode, RunOptions, Session,
  protocol::{Message, Role, ToolCall, ToolSpec, Usage},
  provider::{ChatProvider, ProviderReply},
  run_control,
  tool::{ToolSet, builtins},
};
use async_trait::async_trait;
use serde_json::json;

fn options() -> RunOptions {
  RunOptions {
    instructions: "test".into(),
    permissions: PermissionMode::Auto,
    max_steps: 4,
    max_output_bytes: 1024,
    context_tokens: 16_000,
    compact_at_tokens: 12_000,
    preserve_messages: 4,
    memory_nudge: None,
    budget_usd: None,
    rules: Default::default(),
    hooks: HookRunner::default(),
  }
}

struct ScriptedProvider(Mutex<VecDeque<ProviderReply>>);

#[async_trait]
impl ChatProvider for ScriptedProvider {
  async fn complete(
    &self,
    _messages: &[&Message],
    _tools: &[ToolSpec],
    _events: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    Ok(self.0.lock().unwrap().pop_front().unwrap())
  }
}

#[tokio::test]
async fn agent_runs_tools_until_a_final_message() {
  let temp = tempfile::tempdir().unwrap();
  let replies = VecDeque::from([
    ProviderReply {
      message: Message {
        role: Role::Assistant,
        content: None,
        tool_calls: vec![ToolCall {
          id: "call-1".into(),
          name: "write".into(),
          arguments: json!({"path": "done.txt", "content": "yes"}),
        }],
        tool_call_id: None,
        images: Vec::new(),
        thinking: None,
        response_items: Vec::new(),
        response_model: None,
      },
      usage: Usage {
        input_tokens: 10,
        output_tokens: 2,
        cost_usd: None,
      },
    },
    ProviderReply {
      message: Message::text(Role::Assistant, "done"),
      usage: Usage {
        input_tokens: 15,
        output_tokens: 1,
        cost_usd: None,
      },
    },
  ]);
  let provider = ScriptedProvider(Mutex::new(replies));
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let agent = Agent::new(
    provider,
    tools,
    temp.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "make it".into(), options())
    .await
    .unwrap();

  assert_eq!(output, "done");
  assert_eq!(
    tokio::fs::read_to_string(temp.path().join("done.txt"))
      .await
      .unwrap(),
    "yes"
  );
  assert_eq!(session.messages().unwrap().len(), 4);
}

struct SteeringProvider(Mutex<usize>);

#[async_trait]
impl ChatProvider for SteeringProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    _tools: &[ToolSpec],
    _events: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    let call = {
      let mut calls = self.0.lock().unwrap();
      *calls += 1;
      *calls
    };
    if call == 1 {
      tokio::time::sleep(std::time::Duration::from_millis(20)).await;
      Ok(ProviderReply {
        message: Message::text(Role::Assistant, "first"),
        usage: Usage::default(),
      })
    } else {
      assert!(messages.iter().any(|message| {
        message.role == Role::User && message.content.as_deref() == Some("change direction")
      }));
      Ok(ProviderReply {
        message: Message::text(Role::Assistant, "steered"),
        usage: Usage::default(),
      })
    }
  }
}

#[tokio::test]
async fn steering_is_queued_at_a_safe_conversation_boundary() {
  let temp = tempfile::tempdir().unwrap();
  let agent = Agent::new(
    SteeringProvider(Mutex::new(0)),
    ToolSet::default(),
    temp.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());
  let (controller, mut inbox) = run_control();
  assert!(controller.steer("change direction"));
  let output = agent
    .run_controlled(&mut session, "begin".into(), options(), &mut inbox)
    .await
    .unwrap();
  assert_eq!(output, "steered");
}

struct PendingProvider;

#[async_trait]
impl ChatProvider for PendingProvider {
  async fn complete(
    &self,
    _messages: &[&Message],
    _tools: &[ToolSpec],
    _events: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    std::future::pending().await
  }
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_provider_request() {
  let temp = tempfile::tempdir().unwrap();
  let agent = Agent::new(
    PendingProvider,
    ToolSet::default(),
    temp.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());
  let (controller, mut inbox) = run_control();
  assert!(controller.cancel());
  let error = tokio::time::timeout(
    std::time::Duration::from_millis(100),
    agent.run_controlled(&mut session, "begin".into(), options(), &mut inbox),
  )
  .await
  .unwrap()
  .unwrap_err();
  assert!(error.to_string().contains("run cancelled"));
}

#[tokio::test]
async fn agent_compacts_before_the_context_limit() {
  let temp = tempfile::tempdir().unwrap();
  let provider = ScriptedProvider(Mutex::new(VecDeque::from([
    ProviderReply {
      message: Message::text(Role::Assistant, "earlier work summarized"),
      usage: Usage {
        input_tokens: 80,
        output_tokens: 4,
        cost_usd: None,
      },
    },
    ProviderReply {
      message: Message::text(Role::Assistant, "continued"),
      usage: Usage {
        input_tokens: 30,
        output_tokens: 1,
        cost_usd: None,
      },
    },
  ])));
  let (events, mut receiver) = EventSink::channel();
  let agent = Agent::new(
    provider,
    ToolSet::default(),
    temp.path().into(),
    events,
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());
  for index in 0..4 {
    session.append(Message::text(
      Role::User,
      format!("question {index} {}", "x".repeat(160)),
    ));
    session.append(Message::text(
      Role::Assistant,
      format!("answer {index} {}", "y".repeat(160)),
    ));
  }

  let output = agent
    .run(
      &mut session,
      "continue".into(),
      RunOptions {
        instructions: "test".into(),
        permissions: PermissionMode::ReadOnly,
        max_steps: 2,
        max_output_bytes: 1024,
        context_tokens: 2_000,
        compact_at_tokens: 200,
        preserve_messages: 2,
        budget_usd: None,
        memory_nudge: Some("write down anything durable".into()),
        rules: Default::default(),
        hooks: HookRunner::default(),
      },
    )
    .await
    .unwrap();

  assert_eq!(output, "continued");
  assert_eq!(session.summaries.len(), 1);
  // compaction is the moment the session is asked to save what it worked out
  assert!(
    session
      .messages()
      .unwrap()
      .iter()
      .any(|message| message.content.as_deref() == Some("write down anything durable"))
  );
  let event = receiver.try_recv().unwrap();
  assert!(matches!(event, ainz::Event::Compaction { .. }));
}

fn calls(calls: Vec<ToolCall>) -> ProviderReply {
  ProviderReply {
    message: Message {
      role: Role::Assistant,
      content: None,
      tool_calls: calls,
      tool_call_id: None,
      images: Vec::new(),
      thinking: None,
      response_items: Vec::new(),
      response_model: None,
    },
    usage: Usage::default(),
  }
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
  ToolCall {
    id: id.into(),
    name: name.into(),
    arguments,
  }
}

fn scripted(replies: Vec<ProviderReply>) -> ScriptedProvider {
  ScriptedProvider(Mutex::new(replies.into()))
}

fn tool_results(session: &Session) -> Vec<(String, String)> {
  session
    .messages()
    .unwrap()
    .into_iter()
    .filter(|message| message.role == Role::Tool)
    .map(|message| (message.tool_call_id.unwrap(), message.content.unwrap()))
    .collect()
}

#[tokio::test]
async fn running_out_of_steps_pauses_with_every_call_answered() {
  let temp = tempfile::tempdir().unwrap();
  tokio::fs::write(temp.path().join("a.txt"), "a")
    .await
    .unwrap();
  let read = |id: &str| calls(vec![call(id, "read", json!({"path": "a.txt"}))]);
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let (events, mut receiver) = EventSink::channel();
  let agent = Agent::new(
    scripted(vec![read("1"), read("2")]),
    tools,
    temp.path().into(),
    events,
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());

  let result = agent
    .run(
      &mut session,
      "go".into(),
      RunOptions {
        max_steps: 2,
        ..options()
      },
    )
    .await;

  assert!(result.is_ok(), "{result:?}");
  assert_eq!(tool_results(&session).len(), 2);
  let mut noticed = false;
  while let Ok(event) = receiver.try_recv() {
    noticed |= matches!(event, ainz::Event::Notice { .. });
  }
  assert!(noticed);
}

#[tokio::test]
async fn reads_in_one_reply_keep_their_order_in_the_session() {
  let temp = tempfile::tempdir().unwrap();
  for name in ["a", "b", "c"] {
    tokio::fs::write(temp.path().join(name), name)
      .await
      .unwrap();
  }
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let agent = Agent::new(
    scripted(vec![
      calls(vec![
        call("1", "read", json!({"path": "a"})),
        call("2", "read", json!({"path": "b"})),
        call("3", "write", json!({"path": "d", "content": "d"})),
        call("4", "read", json!({"path": "d"})),
      ]),
      ProviderReply {
        message: Message::text(Role::Assistant, "done"),
        usage: Usage::default(),
      },
    ]),
    tools,
    temp.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());

  agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  let results = tool_results(&session);
  let ids: Vec<_> = results.iter().map(|(id, _)| id.as_str()).collect();
  assert_eq!(ids, ["1", "2", "3", "4"]);
  // the read after the write sees what was written: a write never runs beside its neighbours
  assert!(results[3].1.contains('d'), "{}", results[3].1);
}

#[tokio::test]
async fn arguments_that_did_not_parse_are_answered_not_fatal() {
  let temp = tempfile::tempdir().unwrap();
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let agent = Agent::new(
    scripted(vec![
      calls(vec![call(
        "1",
        "write",
        json!({ainz::protocol::INVALID_ARGUMENTS: "{\"path\": \"x\", \"content\": \"cut"}),
      )]),
      ProviderReply {
        message: Message::text(Role::Assistant, "retried"),
        usage: Usage::default(),
      },
    ]),
    tools,
    temp.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  assert_eq!(output, "retried");
  assert!(tool_results(&session)[0].1.contains("smaller pieces"));
}

struct Panics;

#[async_trait]
impl ainz::tool::Tool for Panics {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "panics".into(),
      description: "always panics".into(),
      parameters: json!({"type": "object"}),
    }
  }

  fn risk(&self, _arguments: &serde_json::Value) -> ainz::tool::Risk {
    ainz::tool::Risk::Read
  }

  async fn execute(
    &self,
    _context: &ainz::tool::ToolContext,
    _arguments: serde_json::Value,
  ) -> anyhow::Result<String> {
    panic!("the tool fell over")
  }
}

#[tokio::test]
async fn a_tool_that_panics_fails_its_call_and_the_run_goes_on() {
  let temp = tempfile::tempdir().unwrap();
  let mut tools = ToolSet::default();
  tools.insert(std::sync::Arc::new(Panics)).unwrap();
  let agent = Agent::new(
    scripted(vec![
      calls(vec![call("1", "panics", json!({}))]),
      ProviderReply {
        message: Message::text(Role::Assistant, "recovered"),
        usage: Usage::default(),
      },
    ]),
    tools,
    temp.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  assert_eq!(output, "recovered");
  assert!(tool_results(&session)[0].1.contains("the tool fell over"));
}
