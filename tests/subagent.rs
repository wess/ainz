use std::sync::{Arc, Mutex};

use ainz::{
  Event, SubagentRegistry, SubagentRequest, SubagentResult,
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
  let tool = subagent_tool(
    SubagentRegistry::new(Arc::new(move |request: SubagentRequest| {
      *captured.lock().unwrap() = Some((request.parent_id, request.prompt));
      let name = request.name;
      Box::pin(async move {
        Ok(SubagentResult {
          session_id: uuid::Uuid::now_v7(),
          name,
          output: "delegated result".into(),
          usage: Usage {
            input_tokens: 4,
            output_tokens: 2,
            cost_usd: None,
          },
        })
      })
    })),
    false,
  );
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
  assert_eq!(output["name"], "shalltear");
  assert_eq!(
    *observed.lock().unwrap(),
    Some((parent, "inspect this".into()))
  );
}

#[tokio::test]
async fn background_delegations_are_collected_by_name() {
  let registry = SubagentRegistry::new(Arc::new(|request: SubagentRequest| {
    let name = request.name;
    Box::pin(async move {
      Ok(SubagentResult {
        session_id: uuid::Uuid::now_v7(),
        name,
        output: "worked".into(),
        usage: Usage::default(),
      })
    })
  }));
  let tool = subagent_tool(registry, true);
  let context = ToolContext {
    workspace: tempfile::tempdir().unwrap().path().into(),
    session_id: uuid::Uuid::now_v7(),
    max_output_bytes: 4096,
  };

  let started = tool
    .execute(
      &context,
      json!({"action": "delegate", "prompt": "look at the logs", "background": true}),
    )
    .await
    .unwrap();
  assert!(started.starts_with("shalltear"), "{started}");

  let collected: Value = serde_json::from_str(
    &tool
      .execute(&context, json!({"action": "collect", "name": "shalltear"}))
      .await
      .unwrap(),
  )
  .unwrap();
  assert_eq!(collected["output"], "worked");
  assert_eq!(collected["name"], "shalltear");

  // a name is only collectable once, and the roster empties with it
  assert!(
    tool
      .execute(&context, json!({"action": "collect", "name": "shalltear"}))
      .await
      .is_err()
  );
  assert_eq!(
    tool
      .execute(&context, json!({"action": "list"}))
      .await
      .unwrap(),
    "no background subagents"
  );
}

#[test]
fn guardian_names_cover_every_floor_then_repeat_with_a_suffix() {
  let first: Vec<_> = (0..10).map(ainz::subagent::guardian).collect();

  assert_eq!(first[0], "shalltear");
  assert_eq!(first[7], "albedo");
  // every name in a run is distinct, which is the point of labelling the roster
  let mut unique = first.clone();
  unique.sort();
  unique.dedup();
  assert_eq!(unique.len(), first.len());
  // past the list the names repeat with a round number rather than colliding
  assert_eq!(ainz::subagent::guardian(10), "shalltear-2");
  assert_eq!(ainz::subagent::guardian(21), "gargantua-3");
}
