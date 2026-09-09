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
  let context = ToolContext::new(tempfile::tempdir().unwrap().path().into(), parent, 4096);
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
  let context = ToolContext::new(
    tempfile::tempdir().unwrap().path().into(),
    uuid::Uuid::now_v7(),
    4096,
  );

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

#[tokio::test]
async fn cancelling_collection_keeps_the_child_addressable() {
  let release = Arc::new(tokio::sync::Notify::new());
  let ready = release.clone();
  let registry = SubagentRegistry::new(Arc::new(move |request| {
    let ready = ready.clone();
    Box::pin(async move {
      ready.notified().await;
      Ok(SubagentResult {
        session_id: uuid::Uuid::now_v7(),
        name: request.name,
        output: "retained".into(),
        usage: Usage::default(),
      })
    })
  }));
  let name = registry.start(SubagentRequest {
    parent_id: uuid::Uuid::nil(),
    name: "worker".into(),
    prompt: "work".into(),
    role: None,
  });
  assert!(
    tokio::time::timeout(
      std::time::Duration::from_millis(20),
      registry.collect(&name)
    )
    .await
    .is_err()
  );
  assert_eq!(registry.running(), vec![(name.clone(), false)]);
  release.notify_one();
  let result = tokio::time::timeout(
    std::time::Duration::from_millis(250),
    registry.collect(&name),
  )
  .await
  .unwrap()
  .unwrap();
  assert_eq!(result.output, "retained");
  assert!(registry.running().is_empty());
}

#[tokio::test]
async fn dropping_the_registry_aborts_the_tasks_it_owns() {
  struct Dropped(Arc<tokio::sync::Notify>);
  impl Drop for Dropped {
    fn drop(&mut self) {
      self.0.notify_one();
    }
  }
  let started = Arc::new(tokio::sync::Notify::new());
  let stopped = Arc::new(tokio::sync::Notify::new());
  let (begin, end) = (started.clone(), stopped.clone());
  let registry = SubagentRegistry::new(Arc::new(move |_| {
    let (begin, end) = (begin.clone(), end.clone());
    Box::pin(async move {
      let _guard = Dropped(end);
      begin.notify_one();
      std::future::pending().await
    })
  }));
  registry.start(SubagentRequest {
    parent_id: uuid::Uuid::nil(),
    name: "worker".into(),
    prompt: "work".into(),
    role: None,
  });
  started.notified().await;
  drop(registry);
  tokio::time::timeout(std::time::Duration::from_millis(250), stopped.notified())
    .await
    .unwrap();
}
