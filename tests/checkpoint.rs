use std::os::unix::fs::PermissionsExt;

use ainz::{
  Session, SessionStore,
  protocol::{Message, Role},
};

#[tokio::test]
async fn concurrent_saves_never_share_a_temporary_file() {
  let root = tempfile::tempdir().unwrap();
  let store = SessionStore::new(root.path().into());
  let mut session = Session::new(root.path().into());
  session.append(Message::text(Role::User, "original"));
  store.save(&session).await.unwrap();
  let updates = (0..32).map(|index| {
    let store = store.clone();
    let mut session = session.clone();
    async move {
      session.append(Message::text(
        Role::Assistant,
        format!("version {index} ").repeat(1024),
      ));
      store.save(&session).await.unwrap();
      let loaded = store.load(session.id).await.unwrap();
      let messages = loaded.messages().unwrap();
      assert_eq!(messages.len(), 2);
      assert_eq!(messages[0].content.as_deref(), Some("original"));
      let text = messages[1].content.as_ref().unwrap();
      assert!((0..32).any(|i| text == &format!("version {i} ").repeat(1024)));
    }
  });
  futures_util::future::join_all(updates).await;
  let entries: Vec<_> = std::fs::read_dir(root.path()).unwrap().collect();
  assert_eq!(entries.len(), 1);
  let mode = entries[0]
    .as_ref()
    .unwrap()
    .metadata()
    .unwrap()
    .permissions()
    .mode();
  assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn a_failed_replacement_cleans_up_only_its_own_temporary_file() {
  let root = tempfile::tempdir().unwrap();
  let store = SessionStore::new(root.path().into());
  let session = Session::new(root.path().into());
  let blocked = root.path().join(format!("{}.json", session.id));
  tokio::fs::create_dir(&blocked).await.unwrap();
  tokio::fs::write(blocked.join("keep"), "untouched")
    .await
    .unwrap();
  tokio::fs::write(root.path().join("unrelated.tmp"), "untouched")
    .await
    .unwrap();
  assert!(
    store
      .save(&session)
      .await
      .unwrap_err()
      .to_string()
      .contains("replace session checkpoint")
  );
  assert_eq!(
    tokio::fs::read_to_string(blocked.join("keep"))
      .await
      .unwrap(),
    "untouched"
  );
  assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}
