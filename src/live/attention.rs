use super::Info;
use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{collections::HashMap, path::Path, process::Stdio, time::Duration};
use tokio::sync::mpsc;
use uuid::Uuid;

pub(crate) fn watch(path: &Path) -> Result<(RecommendedWatcher, mpsc::Receiver<()>)> {
  let (send, receive) = mpsc::channel(1);
  let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
    if event.is_ok_and(|event| {
      matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
      )
    }) {
      let _ = send.try_send(());
    }
  })?;
  watcher.watch(path, RecursiveMode::NonRecursive)?;
  Ok((watcher, receive))
}

pub(crate) struct Monitor {
  _watcher: RecommendedWatcher,
  task: tokio::task::JoinHandle<()>,
  pub updates: mpsc::Receiver<Vec<Info>>,
}
impl Drop for Monitor {
  fn drop(&mut self) {
    self.task.abort();
  }
}

pub(crate) async fn monitor() -> Result<Monitor> {
  let path = super::root()?;
  super::private::dir(&path).await?;
  let (watcher, mut changes) = watch(&path)?;
  let (send, updates) = mpsc::channel(1);
  let task = tokio::spawn(async move {
    loop {
      if let Ok(infos) = super::list().await
        && send.send(infos).await.is_err()
      {
        break;
      }
      if changes.recv().await.is_none() {
        break;
      }
      while changes.try_recv().is_ok() {}
    }
  });
  Ok(Monitor {
    _watcher: watcher,
    task,
    updates,
  })
}

pub(crate) fn terminal(
  infos: &[Info],
  seen: &mut HashMap<Uuid, u64>,
  current: Uuid,
) -> std::io::Result<()> {
  use std::io::Write;
  let alerts = alerts(infos, seen, current);
  if let Some(info) = alerts.first() {
    let suffix = if alerts.len() > 1 {
      format!(" (+{} more)", alerts.len() - 1)
    } else {
      String::new()
    };
    // OSC 9 is handled by supporting outer terminals, including through SSH.
    print!("\x1b]9;{}{}\x07", text(info), suffix);
    std::io::stdout().flush()?;
  }
  Ok(())
}

pub(crate) fn apply(info: &mut Info, message: &super::Message) -> bool {
  let super::Message::State {
    session,
    title,
    label,
    state,
    notifications,
    delivery,
  } = message
  else {
    return false;
  };
  let same = info.session == Some(*session);
  let changed = state != &info.state || !same;
  if !same {
    info.unseen = false;
  }
  if changed {
    info.revision = info.revision.saturating_add(1);
    if same
      && matches!(state.as_str(), "finished run" | "failed run")
      && matches!(info.state.as_str(), "running" | "needs approval")
    {
      info.unseen = !info.viewing;
    } else if state == "running" || state == "ready" {
      info.unseen = false;
    }
  }
  info.label = label.clone();
  info.notifications = *notifications;
  info.delivery = *delivery;
  info.session = Some(*session);
  info.title = title
    .chars()
    .filter(|c| !c.is_control())
    .take(120)
    .collect();
  info.state = state.clone();
  changed
}

pub(crate) fn counts(infos: &[Info], current: Uuid) -> (usize, usize, usize) {
  let mut counts = (0, 0, 0);
  for info in infos
    .iter()
    .filter(|info| info.id != current && info.live())
  {
    match info.attention() {
      Some(0) => counts.0 += 1,
      Some(1) => counts.1 += 1,
      Some(2) => counts.2 += 1,
      _ => {}
    }
  }
  counts
}

pub(crate) fn next(infos: &[Info], current: Uuid) -> Option<Uuid> {
  infos
    .iter()
    .filter(|info| info.live() && info.id != current && !info.attached)
    .filter_map(|info| info.attention().map(|priority| (priority, info.id)))
    .min()
    .map(|(_, id)| id)
}

// The first snapshot seeds the revision map, so attaching never replays old notifications.
pub(crate) fn alerts<'a>(
  infos: &'a [Info],
  seen: &mut HashMap<Uuid, u64>,
  current: Uuid,
) -> Vec<&'a Info> {
  let mut alerts = Vec::new();
  for info in infos {
    let previous = seen.insert(info.id, info.revision);
    if previous.is_some_and(|revision| revision < info.revision)
      && info.id != current
      && !info.attached
      && info.live()
      && info.notifications.accepts(&info.state)
      && info.delivery == ainz::config::NotificationDelivery::Terminal
    {
      alerts.push(info);
    }
  }
  seen.retain(|id, _| infos.iter().any(|info| info.id == *id));
  alerts
}

pub(crate) fn text(info: &Info) -> String {
  format!("Ainz · {} · {}", info.title, info.state)
    .chars()
    .filter(|c| !c.is_control())
    .take(240)
    .collect()
}

pub(crate) async fn system(info: &Info) {
  let text = text(info);
  #[cfg(target_os = "macos")]
  let mut command = {
    let mut command = tokio::process::Command::new("/usr/bin/osascript");
    command.args([
      "-e",
      "on run argv\ndisplay notification (item 1 of argv) with title \"Ainz\"\nend run",
      &text,
    ]);
    command
  };
  #[cfg(not(target_os = "macos"))]
  let mut command = {
    let mut command = tokio::process::Command::new("notify-send");
    command.args(["--app-name=Ainz", "--", "Ainz", &text]);
    command
  };
  command
    .kill_on_drop(true)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
  drop(tokio::time::timeout(Duration::from_secs(2), command.status()).await);
}

#[cfg(test)]
mod tests {
  use super::*;
  use ainz::config::{NotificationDelivery, Notifications};

  fn info(state: &str) -> Info {
    serde_json::from_value(serde_json::json!({
      "id": Uuid::now_v7(), "session": Uuid::now_v7(), "workspace": "/tmp/ainz",
      "title": "reviewer", "state": state, "attached": false,
      "notifications": "all", "revision": 1
    }))
    .unwrap()
  }
  fn state(info: &Info, value: &str) -> super::super::Message {
    super::super::Message::State {
      session: info.session.unwrap(),
      title: info.title.clone(),
      label: info.label.clone(),
      state: value.into(),
      notifications: info.notifications,
      delivery: info.delivery,
    }
  }

  #[test]
  fn completions_are_unseen_until_viewed_and_metadata_changes_do_not_create_more_completions() {
    let mut session = info("running");
    let message = state(&session, "finished run");
    assert!(apply(&mut session, &message));
    assert!(session.unseen);
    assert_eq!(session.badge(), "done · new");
    let revision = session.revision;
    session.unseen = false;
    assert!(!apply(&mut session, &message));
    assert!(!session.unseen);
    assert_eq!(session.revision, revision);
    assert_eq!(session.badge(), "ready");
  }

  #[test]
  fn an_open_picker_does_not_mark_a_completion_seen_but_a_viewed_session_does() {
    let mut session = info("running");
    session.attached = true;
    let message = state(&session, "failed run");
    apply(&mut session, &message);
    assert!(session.unseen);
    session.viewing = true;
    apply(&mut session, &state(&info("ready"), "ready"));
    let message = state(&session, "running");
    apply(&mut session, &message);
    let message = state(&session, "finished run");
    apply(&mut session, &message);
    assert!(!session.unseen);
  }

  #[test]
  fn attention_prefers_approvals_then_unseen_failures_then_unseen_completions() {
    let current = Uuid::now_v7();
    let mut done = info("finished run");
    done.unseen = true;
    let mut failed = info("failed run");
    failed.unseen = true;
    let mut blocked = info("needs approval");
    assert_eq!(
      next(&[done.clone(), failed.clone(), blocked.clone()], current),
      Some(blocked.id)
    );
    blocked.attached = true;
    assert_eq!(
      next(&[done.clone(), failed.clone(), blocked.clone()], current),
      Some(failed.id)
    );
    assert_eq!(counts(&[done.clone(), failed, blocked], current), (1, 1, 1));
    assert_eq!(next(&[done.clone()], done.id), None);
    done.unseen = false;
    assert_eq!(next(&[done], current), None);
  }

  #[test]
  fn notifications_do_not_replay_or_repeat_and_respect_the_event_and_delivery_settings() {
    let current = Uuid::now_v7();
    let mut session = info("running");
    let mut seen = HashMap::new();
    assert!(alerts(&[session.clone()], &mut seen, current).is_empty());
    session.state = "needs approval".into();
    session.revision += 1;
    assert_eq!(alerts(&[session.clone()], &mut seen, current).len(), 1);
    assert!(alerts(&[session.clone()], &mut seen, current).is_empty());
    session.state = "finished run".into();
    session.revision += 1;
    session.notifications = Notifications::NeedsInput;
    assert!(alerts(&[session.clone()], &mut seen, current).is_empty());
    session.state = "needs approval".into();
    session.revision += 1;
    session.delivery = NotificationDelivery::System;
    assert!(alerts(&[session.clone()], &mut seen, current).is_empty());
    session.delivery = NotificationDelivery::Terminal;
    session.revision += 1;
    session.attached = true;
    assert!(alerts(&[session.clone()], &mut seen, current).is_empty());
    session.attached = false;
    session.revision += 1;
    assert!(alerts(&[session.clone()], &mut seen, session.id).is_empty());
    session.notifications = Notifications::Off;
    session.revision += 1;
    assert!(alerts(&[session], &mut seen, current).is_empty());
  }
}
