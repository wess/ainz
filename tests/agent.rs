use std::{collections::VecDeque, sync::Mutex};

use ainz::{
  Agent, EventSink, PermissionMode, RunOptions, Session,
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
  }
}

struct ScriptedProvider(Mutex<VecDeque<ProviderReply>>);

#[async_trait]
impl ChatProvider for ScriptedProvider {
  async fn complete(
    &self,
    _messages: &[Message],
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
    messages: &[Message],
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
    _messages: &[Message],
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
        memory_nudge: Some("write down anything durable".into()),
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
