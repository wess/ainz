use std::path::PathBuf;

use ainz::{
  Session, SessionStore,
  protocol::{Message, Role, ToolCall, Usage},
};
use serde_json::json;

#[tokio::test]
async fn session_history_branches_and_round_trips() {
  let temp = tempfile::tempdir().unwrap();
  let store = SessionStore::new(temp.path().join("sessions"));
  let mut session = Session::new(PathBuf::from("/workspace"));
  let root = session.append(Message::text(Role::User, "first"));
  session.append(Message::text(Role::Assistant, "original"));
  session.checkout(Some(root)).unwrap();
  session.append(Message::text(Role::User, "branch"));

  let messages = session.messages().unwrap();
  assert_eq!(messages.len(), 2);
  assert_eq!(messages[1].content.as_deref(), Some("branch"));

  store.save(&session).await.unwrap();
  let loaded = store.load(session.id).await.unwrap();
  assert_eq!(loaded.messages().unwrap(), messages);
}

#[test]
fn compaction_is_branch_aware() {
  let mut session = Session::new(PathBuf::from("/workspace"));
  let root = session.append(Message::text(Role::User, "root"));
  for index in 0..5 {
    session.append(Message::text(Role::Assistant, format!("answer {index}")));
    session.append(Message::text(Role::User, format!("question {index}")));
  }
  let (cursor, input, archived) = session.compaction_input(3).unwrap().unwrap();
  assert!(archived >= 2);
  assert!(!input.is_empty());
  session
    .record_summary(cursor, "durable summary".into())
    .unwrap();

  let context = session.context_messages().unwrap();
  assert_eq!(context[0].role, Role::System);
  assert!(
    context[0]
      .content
      .as_deref()
      .unwrap()
      .contains("durable summary")
  );

  session.checkout(Some(root)).unwrap();
  let branch = session.context_messages().unwrap();
  assert_eq!(branch.len(), 1);
  assert_eq!(branch[0].role, Role::User);
}

#[tokio::test]
async fn sessions_are_searchable_after_the_fact() {
  let temp = tempfile::tempdir().unwrap();
  let store = SessionStore::new(temp.path().join("sessions"));
  let mut session = Session::new(PathBuf::from("/workspace"));
  session.append(Message::text(
    Role::User,
    "the deploy failed with a certificate error on gohan",
  ));
  session.append(Message::text(
    Role::Assistant,
    "the intermediate chain was missing; concatenating it fixed the handshake",
  ));
  store.save(&session).await.unwrap();

  let mut elsewhere = Session::new(PathBuf::from("/other"));
  elsewhere.append(Message::text(Role::User, "certificate error"));
  store.save(&elsewhere).await.unwrap();

  let found = store
    .search("certificate gohan", Some(&PathBuf::from("/workspace")), 5)
    .await
    .unwrap();
  assert_eq!(found.len(), 1);
  assert_eq!(found[0].id, session.id);
  assert_eq!(found[0].score, 2);
  assert!(found[0].excerpts[0].starts_with("user: "));

  // the other workspace is only reachable when the search asks for everything
  let everywhere = store.search("certificate", None, 5).await.unwrap();
  assert_eq!(everywhere.len(), 2);
  assert!(store.search("   ", None, 5).await.is_err());
}

#[tokio::test]
async fn search_survives_characters_whose_case_changes_length() {
  let temp = tempfile::tempdir().unwrap();
  let store = SessionStore::new(temp.path().join("sessions"));
  let mut session = Session::new(PathBuf::from("/workspace"));
  // 'İ' lowercases to two code points, so offsets taken from a lowercased copy run ahead of
  // the original and cannot be used to slice it
  session.append(Message::text(
    Role::User,
    format!("{} the GOHAN deploy failed", "İ".repeat(40)),
  ));
  store.save(&session).await.unwrap();

  let found = store.search("gohan", None, 5).await.unwrap();

  assert_eq!(found.len(), 1);
  assert!(
    found[0].excerpts[0].contains("GOHAN"),
    "{:?}",
    found[0].excerpts
  );
}

#[test]
fn export_covers_the_active_path_and_drops_a_rewound_branch() {
  let mut session = Session::new(PathBuf::from("/workspace"));
  let root = session.append(Message::text(Role::User, "what broke the deploy"));
  // this branch gets abandoned by the checkout below and must not appear in the export
  session.append(Message::text(Role::Assistant, "abandoned guess about DNS"));

  session.checkout(Some(root)).unwrap();
  let mut answer = Message::text(Role::Assistant, "checking the certificate chain");
  answer.tool_calls.push(ToolCall {
    id: "call-1".into(),
    name: "read_file".into(),
    arguments: json!({"path": "deploy.log"}),
  });
  session.append(answer);
  session.append(Message::tool("call-1", "certificate expired 2 days ago"));

  let markdown = session.export_markdown().unwrap();

  assert!(markdown.contains(&format!("# Session {}", session.id)));
  assert!(markdown.contains("/workspace"));
  assert!(markdown.contains("## User"));
  assert!(markdown.contains("what broke the deploy"));
  assert!(markdown.contains("## Assistant"));
  assert!(markdown.contains("checking the certificate chain"));
  assert!(markdown.contains("### Tool call: read_file"));
  assert!(markdown.contains("deploy.log"));
  assert!(markdown.contains("## Tool result (`call-1`)"));
  assert!(markdown.contains("certificate expired 2 days ago"));

  assert!(!markdown.contains("abandoned guess about DNS"));
}

#[tokio::test]
async fn usage_cost_round_trips_through_a_saved_session() {
  let temp = tempfile::tempdir().unwrap();
  let store = SessionStore::new(temp.path().join("sessions"));

  let mut priced = Session::new(PathBuf::from("/workspace"));
  priced.append(Message::text(Role::User, "hi"));
  priced.usage.cost_usd = Some(0.0842);
  store.save(&priced).await.unwrap();
  let loaded = store.load(priced.id).await.unwrap();
  assert_eq!(loaded.usage.cost_usd, Some(0.0842));

  let mut unpriced = Session::new(PathBuf::from("/workspace"));
  unpriced.append(Message::text(Role::User, "hi"));
  store.save(&unpriced).await.unwrap();
  let loaded = store.load(unpriced.id).await.unwrap();
  assert_eq!(loaded.usage.cost_usd, None);
  // no cost known, so the field is left out rather than written as null
  assert!(
    !serde_json::to_string(&loaded.usage)
      .unwrap()
      .contains("cost")
  );
}

#[test]
fn usage_without_cost_deserializes_from_a_session_saved_before_this_field_existed() {
  let usage: Usage = serde_json::from_str(r#"{"input_tokens":10,"output_tokens":2}"#).unwrap();
  assert_eq!(usage.input_tokens, 10);
  assert_eq!(usage.cost_usd, None);
}
