use std::{sync::Arc, time::Duration};

use ainz::{
  Agent, ChatProvider, Event, EventSink, RunOptions, Session,
  control::{MAX_STEERING_BYTES, MAX_STEERING_MESSAGES},
  protocol::{Message, Role, ToolSpec},
  provider::ProviderReply,
  tool::ToolSet,
};
use async_trait::async_trait;
use tokio::sync::Notify;

struct Pending(Arc<Notify>);

#[async_trait]
impl ChatProvider for Pending {
  async fn complete(
    &self,
    _: &[Message],
    _: &[ToolSpec],
    _: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    self.0.notify_one();
    std::future::pending().await
  }
}

#[test]
fn steering_rejects_empty_oversized_and_full_queues() {
  let (controller, inbox) = ainz::run_control();
  assert!(!controller.steer("  "));
  assert!(!controller.steer("é".repeat(MAX_STEERING_BYTES / 2 + 1)));
  for _ in 0..MAX_STEERING_MESSAGES {
    assert!(controller.steer("x".repeat(MAX_STEERING_BYTES)));
  }
  assert!(!controller.steer("overflow"));
  assert!(controller.cancel());
  assert!(controller.cancel());
  assert!(!controller.steer("after cancel"));
  drop(inbox);
  assert!(!controller.cancel());
}

#[tokio::test]
async fn cancellation_bypasses_a_full_queue_during_a_provider_wait() {
  let root = tempfile::tempdir().unwrap();
  let started = Arc::new(Notify::new());
  let (events, mut rx) = EventSink::channel();
  let agent = Agent::new(
    Pending(started.clone()),
    ToolSet::default(),
    root.path().into(),
    events,
    ainz::deny_all(),
  );
  let (controller, mut inbox) = ainz::run_control();
  let mut session = Session::new(root.path().into());
  let run = agent.run_controlled(
    &mut session,
    "begin".into(),
    RunOptions::default(),
    &mut inbox,
  );
  let cancel = async {
    started.notified().await;
    for index in 0..MAX_STEERING_MESSAGES {
      assert!(controller.steer(format!("message {index}")));
    }
    assert!(!controller.steer("overflow"));
    assert!(controller.cancel());
  };
  let (result, ()) = tokio::time::timeout(Duration::from_millis(250), async {
    tokio::join!(run, cancel)
  })
  .await
  .unwrap();
  assert!(result.unwrap_err().to_string().contains("run cancelled"));
  assert!(matches!(rx.try_recv().unwrap(), Event::Cancelled));
  assert!(rx.try_recv().is_err());
  assert_eq!(session.messages().unwrap().len(), 1);
}

struct Echo;

#[async_trait]
impl ChatProvider for Echo {
  async fn complete(
    &self,
    _: &[Message],
    _: &[ToolSpec],
    _: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    tokio::time::sleep(Duration::from_millis(5)).await;
    Ok(ProviderReply {
      message: Message::text(Role::Assistant, "done"),
      usage: Default::default(),
    })
  }
}

#[tokio::test]
async fn dropping_every_controller_does_not_cancel_or_spin_the_run() {
  let root = tempfile::tempdir().unwrap();
  let (controller, mut inbox) = ainz::run_control();
  drop(controller);
  let agent = Agent::new(
    Echo,
    ToolSet::default(),
    root.path().into(),
    EventSink::default(),
    ainz::deny_all(),
  );
  let mut session = Session::new(root.path().into());
  let output = tokio::time::timeout(
    Duration::from_millis(250),
    agent.run_controlled(
      &mut session,
      "begin".into(),
      RunOptions::default(),
      &mut inbox,
    ),
  )
  .await
  .unwrap()
  .unwrap();
  assert_eq!(output, "done");
}
