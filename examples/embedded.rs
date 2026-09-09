use std::sync::Arc;

use ainz::{
  Agent, ChatProvider, Event, EventSink, RunOptions, Session, deny_all,
  protocol::{Message, Role, ToolCall, ToolSpec},
  provider::ProviderReply,
  tool::{Risk, Tool, ToolContext, ToolSet},
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};

// a deterministic host provider keeps the example runnable without an account or network
struct Provider;

#[async_trait]
impl ChatProvider for Provider {
  async fn complete(
    &self,
    messages: &[Message],
    _tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    let last = messages.last().unwrap();
    let mut message = Message::text(Role::Assistant, "");
    if last.role == Role::Tool {
      let text = last.content.clone().unwrap_or_default();
      events.emit(Event::TextDelta { text: text.clone() });
      message.content = Some(text);
    } else {
      message.tool_calls.push(ToolCall {
        id: uuid::Uuid::now_v7().to_string(),
        name: "lookup".into(),
        arguments: json!({}),
      });
    }
    Ok(ProviderReply {
      message,
      usage: Default::default(),
    })
  }
}

struct Lookup;

#[async_trait]
impl Tool for Lookup {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "lookup".into(),
      description: "Read a value owned by the host".into(),
      parameters: json!({"type": "object", "properties": {}}),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Read
  }

  async fn execute(&self, _context: &ToolContext, _arguments: Value) -> Result<String> {
    Ok("host value".into())
  }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  let workspace = std::env::current_dir()?;
  let events = EventSink::new(|event| {
    if let Event::TextDelta { text } = event {
      println!("text: {text}");
    }
  });
  let mut tools = ToolSet::default();
  tools.insert(Arc::new(Lookup))?;
  let agent = Agent::new(Provider, tools, workspace.clone(), events, deny_all());
  let mut session = Session::new(workspace);
  let options = RunOptions {
    instructions: "Use lookup to answer.".into(),
    ..Default::default()
  };
  let reply = agent
    .run(&mut session, "Read the host value".into(), options.clone())
    .await?;
  ensure!(reply == "host value", "unexpected reply");

  // the host owns persistence; no default config, state directory, or plugin discovery is used
  let checkpoint = serde_json::to_vec(&session)?;
  let mut restored: Session = serde_json::from_slice(&checkpoint)?;
  let reply = agent
    .run(&mut restored, "Read it again".into(), options)
    .await?;
  ensure!(
    reply == "host value" && restored.messages()?.len() == 8,
    "resume failed"
  );
  println!("completed: custom tool, streamed reply, checkpoint, resume");
  Ok(())
}
