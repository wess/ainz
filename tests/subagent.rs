use std::sync::{Arc, Mutex};

use agentx::{
  Event, SubagentResult,
  protocol::Usage,
  subagent_tool,
  tool::{Risk, ToolContext},
};
use serde_json::{Value, json};

#[test]
fn subagent_events_keep_their_session_identity() {
  let event = Event::SubagentEvent {
    session_id: "child-1".into(),
    event: Box::new(Event::TextDelta {
      text: "working".into(),
    }),
  };
  let value = serde_json::to_value(event).unwrap();

  assert_eq!(value["type"], "subagent_event");
  assert_eq!(value["session_id"], "child-1");
  assert_eq!(value["event"]["type"], "text_delta");
  assert_eq!(value["event"]["text"], "working");
}

#[tokio::test]
async fn subagent_tools_receive_the_parent_and_return_child_metadata() {
  let parent = uuid::Uuid::now_v7();
  let observed = Arc::new(Mutex::new(None));
  let captured = observed.clone();
  let tool = subagent_tool(Arc::new(move |parent_id, prompt| {
    *captured.lock().unwrap() = Some((parent_id, prompt));
    Box::pin(async move {
      Ok(SubagentResult {
        session_id: uuid::Uuid::now_v7(),
        output: "delegated result".into(),
        usage: Usage {
          input_tokens: 4,
          output_tokens: 2,
        },
      })
    })
  }));
  let context = ToolContext {
    workspace: tempfile::tempdir().unwrap().path().into(),
    session_id: parent,
    max_output_bytes: 4096,
  };
  assert_eq!(tool.risk(&json!({})), Risk::Execute);
  let output: Value = serde_json::from_str(
    &tool
      .execute(&context, json!({"prompt": "inspect this"}))
      .await
      .unwrap(),
  )
  .unwrap();
  assert_eq!(output["output"], "delegated result");
  assert_eq!(
    *observed.lock().unwrap(),
    Some((parent, "inspect this".into()))
  );
}
