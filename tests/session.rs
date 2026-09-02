use std::path::PathBuf;

use ainz::{
  Session, SessionStore,
  protocol::{Message, Role},
};

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
