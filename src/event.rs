use std::sync::Arc;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::protocol::{ToolCall, Usage};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
  SessionStart {
    session_id: String,
  },
  TextDelta {
    text: String,
  },
  ToolStart {
    call: ToolCall,
  },
  ToolEnd {
    id: String,
    output: String,
    error: bool,
  },
  TurnEnd {
    usage: Usage,
  },
  Compaction {
    archived_messages: usize,
    summary: String,
  },
  Steering {
    message: String,
  },
  Cancelled,
  SubagentStart {
    session_id: String,
    parent_id: String,
    name: String,
  },
  SubagentEnd {
    session_id: String,
    error: bool,
  },
  SubagentEvent {
    session_id: String,
    event: Box<Event>,
  },
  Error {
    message: String,
  },
}

#[derive(Clone, Default)]
pub struct EventSink(Option<Arc<dyn Fn(Event) + Send + Sync>>);

impl EventSink {
  pub fn new(f: impl Fn(Event) + Send + Sync + 'static) -> Self {
    Self(Some(Arc::new(f)))
  }

  pub fn channel() -> (Self, mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Self::new(move |event| drop(tx.send(event))), rx)
  }

  pub fn emit(&self, event: Event) {
    if let Some(f) = &self.0 {
      f(event);
    }
  }
}
