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
