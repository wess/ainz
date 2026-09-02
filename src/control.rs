use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct RunController {
  sender: mpsc::UnboundedSender<RunSignal>,
}

#[derive(Debug)]
pub struct RunInbox {
  receiver: mpsc::UnboundedReceiver<RunSignal>,
  closed: bool,
}

#[derive(Debug)]
pub(crate) enum RunSignal {
  Steer(String),
  Cancel,
}

pub fn run_control() -> (RunController, RunInbox) {
  let (sender, receiver) = mpsc::unbounded_channel();
  (
    RunController { sender },
    RunInbox {
      receiver,
      closed: false,
    },
  )
}

impl RunController {
  pub fn steer(&self, message: impl Into<String>) -> bool {
    self.sender.send(RunSignal::Steer(message.into())).is_ok()
  }

  pub fn cancel(&self) -> bool {
    self.sender.send(RunSignal::Cancel).is_ok()
  }
}

impl RunInbox {
  pub(crate) async fn receive(&mut self) -> Option<RunSignal> {
    if self.closed {
      return None;
    }
    let signal = self.receiver.recv().await;
    self.closed = signal.is_none();
    signal
  }

  pub(crate) fn is_open(&self) -> bool {
    !self.closed
  }

  pub(crate) fn try_receive(&mut self) -> Option<RunSignal> {
    match self.receiver.try_recv() {
      Ok(signal) => Some(signal),
      Err(mpsc::error::TryRecvError::Disconnected) => {
        self.closed = true;
        None
      }
      Err(mpsc::error::TryRecvError::Empty) => None,
    }
  }
}
