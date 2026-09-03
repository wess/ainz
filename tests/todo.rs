use ainz::{TodoList, tool::ToolContext};
use serde_json::json;

fn context() -> ToolContext {
  ToolContext {
    workspace: std::env::temp_dir(),
    session_id: uuid::Uuid::now_v7(),
    max_output_bytes: 8192,
  }
}

#[test]
fn setting_a_list_starts_everything_pending() {
  let list = TodoList::new();
  list.set(vec!["write tests".into(), "run clippy".into()]);
  let rendered = list.render();
  assert_eq!(rendered, "1. [ ] write tests\n2. [ ] run clippy");
}

#[test]
fn starting_an_item_returns_the_previous_one_to_pending() {
  let list = TodoList::new();
  list.set(vec!["a".into(), "b".into(), "c".into()]);
  list.start("a").unwrap();
  assert!(list.render().contains("[>] a"));

  // starting b un-starts a: only one item is ever doing
  list.start("2").unwrap();
  let rendered = list.render();
  assert!(rendered.contains("[ ] a"), "{rendered}");
  assert!(rendered.contains("[>] b"), "{rendered}");
  assert!(rendered.contains("[ ] c"), "{rendered}");
}

#[test]
fn marking_an_item_done() {
  let list = TodoList::new();
  list.set(vec!["a".into(), "b".into()]);
  list.start("a").unwrap();
  list.done("a").unwrap();
  let rendered = list.render();
  assert!(rendered.contains("[x] a"), "{rendered}");
  assert!(rendered.contains("[ ] b"), "{rendered}");
}

#[test]
fn listing_an_empty_plan() {
  let list = TodoList::new();
  assert_eq!(list.render(), "(no plan set)");
}

#[test]
fn an_out_of_range_index_gives_a_clear_error() {
  let list = TodoList::new();
  list.set(vec!["only one".into()]);
  let error = list.start("5").unwrap_err().to_string();
  assert!(error.contains('5'), "{error}");
  assert!(error.to_lowercase().contains("index"), "{error}");

  let missing = list.done("nothing like this").unwrap_err().to_string();
  assert!(missing.contains("no item matches"), "{missing}");
}

#[test]
fn the_rendered_output_shows_all_three_markers() {
  let list = TodoList::new();
  list.set(vec!["a".into(), "b".into(), "c".into()]);
  list.start("a").unwrap();
  list.done("a").unwrap();
  list.start("b").unwrap();
  let rendered = list.render();
  assert!(rendered.contains("[x]"), "{rendered}");
  assert!(rendered.contains("[>]"), "{rendered}");
  assert!(rendered.contains("[ ]"), "{rendered}");
}

#[tokio::test]
async fn the_tool_returns_the_whole_list_after_each_action() {
  let list = TodoList::new();
  let tool = list.tool();
  let context = context();

  let after_set = tool
    .execute(
      &context,
      json!({"action": "set", "items": ["draft", "review", "ship"]}),
    )
    .await
    .unwrap();
  assert_eq!(after_set, "1. [ ] draft\n2. [ ] review\n3. [ ] ship");

  let after_start = tool
    .execute(&context, json!({"action": "start", "target": "draft"}))
    .await
    .unwrap();
  assert!(after_start.contains("[>] draft"), "{after_start}");

  let after_done = tool
    .execute(&context, json!({"action": "done", "target": "1"}))
    .await
    .unwrap();
  assert!(after_done.contains("[x] draft"), "{after_done}");

  let listed = tool
    .execute(&context, json!({"action": "list"}))
    .await
    .unwrap();
  assert_eq!(listed, after_done);

  let error = tool
    .execute(&context, json!({"action": "start", "target": "9"}))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("no item at index"), "{error}");
}
