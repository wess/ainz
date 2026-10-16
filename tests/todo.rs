use ainz::{
  Event, EventSink, Todo, TodoStatus,
  protocol::ToolCall,
  todo::{self, planned},
  tool::ToolContext,
};
use serde_json::json;

fn context(events: EventSink) -> ToolContext {
  let mut context = ToolContext::new(std::env::temp_dir(), uuid::Uuid::now_v7(), 8192);
  context.progress = Some((events, "call".into()));
  context
}

#[tokio::test]
async fn every_call_publishes_the_whole_plan() {
  let (events, mut rx) = EventSink::channel();
  let output = todo::tool()
    .execute(
      &context(events),
      json!({"todos": [
        {"text": "write tests", "status": "done"},
        {"text": "run clippy", "status": "in_progress"},
        {"text": "ship", "status": "pending"}
      ]}),
    )
    .await
    .unwrap();

  assert_eq!(output, "1/3 done");
  let Ok(Event::Todos { items }) = rx.try_recv() else {
    panic!("no plan was published");
  };
  assert_eq!(items.len(), 3);
  assert_eq!(items[1].status, TodoStatus::InProgress);
}

#[test]
fn two_items_in_progress_is_refused() {
  let tool = todo::tool();
  let error = tool
    .validate(&json!({"todos": [
      {"text": "a", "status": "in_progress"},
      {"text": "b", "status": "in_progress"}
    ]}))
    .unwrap_err()
    .to_string();
  assert!(error.contains("exactly one"), "{error}");
}

#[test]
fn an_item_without_text_is_refused() {
  let error = todo::tool()
    .validate(&json!({"todos": [{"text": "  ", "status": "pending"}]}))
    .unwrap_err()
    .to_string();
  assert!(error.contains("text"), "{error}");
}

#[test]
fn the_plan_is_read_back_from_the_call_that_set_it() {
  let call = ToolCall {
    id: "1".into(),
    name: "todo".into(),
    arguments: json!({"todos": [{"text": "a", "status": "pending"}]}),
  };
  assert_eq!(
    planned(&call),
    Some(vec![Todo {
      text: "a".into(),
      status: TodoStatus::Pending,
    }])
  );
  let other = ToolCall {
    name: "shell".into(),
    ..call
  };
  assert_eq!(planned(&other), None);
}
