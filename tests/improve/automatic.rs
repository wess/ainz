use ainz::{
  Session,
  improve::{
    ImproveConfig,
    automatic::{AutomaticStore, Outcome},
  },
};

#[test]
fn new_runs_are_deduplicated_and_the_schedule_survives_restarts() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join("automatic");
  let store = AutomaticStore { root: root.clone() };
  let policy = ImproveConfig {
    enabled: true,
    every_runs: 2,
    ..Default::default()
  };
  let mut session = Session::new(temp.path().join("workspace"));
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::User,
    "first",
  ));
  assert!(store.note(&session, &policy).unwrap().is_none());
  assert!(store.note(&session, &policy).unwrap().is_none());
  let store = AutomaticStore { root };
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::Assistant,
    "second",
  ));
  let job = store.note(&session, &policy).unwrap().unwrap();
  store.started(job.id, std::process::id()).unwrap();
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::User,
    "third",
  ));
  assert!(store.note(&session, &policy).unwrap().is_none());
  store
    .finished(
      &job,
      Outcome {
        id: job.id,
        experiment: None,
        error: Some("fixture".into()),
      },
    )
    .unwrap();
  assert!(
    store
      .notice(&session.workspace)
      .unwrap()
      .unwrap()
      .contains("fixture")
  );
  assert!(store.notice(&session.workspace).unwrap().is_none());
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::Assistant,
    "fourth",
  ));
  assert!(store.note(&session, &policy).unwrap().is_none());
  assert_eq!(store.read().unwrap().projects[&session.workspace].runs, 2);
}

#[test]
fn disabled_and_child_sessions_do_not_schedule_and_dead_workers_do_not_block() {
  let temp = tempfile::tempdir().unwrap();
  let store = AutomaticStore {
    root: temp.path().join("automatic"),
  };
  let mut session = Session::new(temp.path().join("workspace"));
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::User,
    "run",
  ));
  assert!(
    store
      .note(&session, &ImproveConfig::default())
      .unwrap()
      .is_none()
  );
  assert!(!store.root.exists());
  let policy = ImproveConfig {
    enabled: true,
    every_runs: 1,
    cooldown_seconds: 0,
    ..Default::default()
  };
  let mut child = session.clone();
  child.parent_id = Some(uuid::Uuid::now_v7());
  assert!(store.note(&child, &policy).unwrap().is_none());
  let first = store.note(&session, &policy).unwrap().unwrap();
  store.started(first.id, i32::MAX as u32).unwrap();
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::User,
    "next",
  ));
  let next = store.note(&session, &policy).unwrap().unwrap();
  assert_ne!(first.id, next.id);
  // a late report from the old worker cannot replace the current result or reservation
  store
    .finished(
      &first,
      Outcome {
        id: first.id,
        experiment: None,
        error: Some("late".into()),
      },
    )
    .unwrap();
  assert_eq!(store.read().unwrap().active.unwrap().id, next.id);
}

#[test]
fn concurrent_surfaces_cannot_start_duplicate_experiments() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join("automatic");
  let mut session = Session::new(temp.path().join("workspace"));
  session.append(ainz::protocol::Message::text(
    ainz::protocol::Role::User,
    "run",
  ));
  let mut threads = Vec::new();
  for _ in 0..4 {
    let root = root.clone();
    let session = session.clone();
    threads.push(std::thread::spawn(move || {
      let store = AutomaticStore { root };
      store
        .note(
          &session,
          &ImproveConfig {
            enabled: true,
            every_runs: 1,
            ..Default::default()
          },
        )
        .unwrap()
        .is_some()
    }));
  }
  assert_eq!(
    threads
      .into_iter()
      .map(|t| t.join().unwrap())
      .filter(|started| *started)
      .count(),
    1
  );
}
