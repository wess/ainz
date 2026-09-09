use std::{
  collections::VecDeque,
  sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use tokio::sync::watch;

pub const MAX_STEERING_MESSAGES: usize = 32;
pub const MAX_STEERING_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct Steering {
  open: bool,
  messages: VecDeque<String>,
}

type Queue = Arc<Mutex<Steering>>;

#[derive(Clone, Debug)]
pub struct RunController {
  queue: Queue,
  cancel: watch::Sender<bool>,
}

#[derive(Debug)]
pub struct RunInbox {
  queue: Queue,
  cancel: watch::Receiver<bool>,
}

fn lock(queue: &Queue) -> MutexGuard<'_, Steering> {
  queue.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates control for one run. Cancellation is sticky; use a fresh pair for the next run.
pub fn run_control() -> (RunController, RunInbox) {
  let queue = Arc::new(Mutex::new(Steering {
    open: true,
    messages: VecDeque::new(),
  }));
  let (cancel, cancelled) = watch::channel(false);
  (
    RunController {
      queue: queue.clone(),
      cancel,
    },
    RunInbox {
      queue,
      cancel: cancelled,
    },
  )
}

impl RunController {
  /// Returns false for empty/oversized messages, a full queue, or a cancelled/closed run.
  pub fn steer(&self, message: impl Into<String>) -> bool {
    self.try_steer(message).is_ok()
  }

  /// Queues steering or explains why it could not be accepted.
  pub fn try_steer(&self, message: impl Into<String>) -> Result<(), &'static str> {
    let message = message.into();
    if message.len() > MAX_STEERING_BYTES {
      return Err("message exceeds 64 KiB");
    }
    if message.trim().is_empty() {
      return Err("message is empty");
    }
    if *self.cancel.borrow() {
      return Err("run was cancelled");
    }
    let mut queue = lock(&self.queue);
    if !queue.open {
      return Err("run is finishing or closed");
    }
    if queue.messages.len() >= MAX_STEERING_MESSAGES {
      return Err("steering queue is full (32 messages)");
    }
    queue
      .messages
      .push_back(message.into_boxed_str().into_string());
    Ok(())
  }

  /// Cancellation has its own signal and never waits behind queued steering.
  pub fn cancel(&self) -> bool {
    self.cancel.send(true).is_ok()
  }
}

impl RunInbox {
  pub(crate) fn cancellation(&self) -> watch::Receiver<bool> {
    self.cancel.clone()
  }

  pub(crate) fn drain(&mut self, final_reply: bool) -> Vec<String> {
    let mut queue = lock(&self.queue);
    // deciding to finish and closing steering must be atomic with respect to senders
    if final_reply && queue.messages.is_empty() {
      queue.open = false;
    }
    queue.messages.drain(..).collect()
  }

  pub(crate) fn close(&mut self) {
    let mut queue = lock(&self.queue);
    queue.open = false;
    queue.messages.clear();
  }
}

impl Drop for RunInbox {
  fn drop(&mut self) {
    self.close();
  }
}

pub(crate) async fn cancelled(signal: Option<watch::Receiver<bool>>) {
  if let Some(mut signal) = signal {
    loop {
      if *signal.borrow_and_update() {
        return;
      }
      if signal.changed().await.is_err() {
        break;
      }
    }
  }
  // dropping the controller detaches control; it does not cancel or spin the run
  std::future::pending().await
}
