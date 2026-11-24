use ainz::{Event, EventSink, Session, routing::ModelTarget, tool::truncate};
use anyhow::Result;

/// Keeps the roster informed even if the worker future is dropped during setup or execution.
pub(super) struct WorkerRun {
  events: EventSink,
  id: String,
  finished: bool,
}

impl WorkerRun {
  pub fn start(
    events: EventSink,
    session: &Session,
    parent_id: uuid::Uuid,
    name: &str,
    task: &str,
    target: ModelTarget,
  ) -> Self {
    let id = session.id.to_string();
    events.emit(Event::SubagentStart {
      session_id: id.clone(),
      parent_id: parent_id.to_string(),
      name: name.into(),
      task: task.into(),
      target,
    });
    Self {
      events,
      id,
      finished: false,
    }
  }

  pub fn sink(&self) -> EventSink {
    let events = self.events.clone();
    let id = self.id.clone();
    EventSink::new(move |event| {
      events.emit(Event::SubagentEvent {
        session_id: id.clone(),
        event: Box::new(event),
      });
    })
  }

  pub fn finish(mut self, result: &Result<String>) {
    if let Err(error) = result {
      self.sink().emit(Event::Error {
        message: truncate(format!("{error:#}"), 1200),
      });
    }
    self.finished = true;
    self.events.emit(Event::SubagentEnd {
      session_id: self.id.clone(),
      error: result.is_err(),
    });
  }
}

impl Drop for WorkerRun {
  fn drop(&mut self) {
    if !self.finished {
      self.sink().emit(if std::thread::panicking() {
        Event::Error {
          message: "The worker stopped unexpectedly.".into(),
        }
      } else {
        Event::Cancelled
      });
      self.events.emit(Event::SubagentEnd {
        session_id: self.id.clone(),
        error: true,
      });
    }
  }
}
