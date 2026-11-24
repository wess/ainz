use std::{
  collections::{BTreeMap, HashSet},
  sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;
use uuid::Uuid;

use crate::{
  event::{Event, EventSink, WorkerStatus},
  tool::truncate,
};

use super::stream::{blocks, field};

static NEXT_WORKER: AtomicUsize = AtomicUsize::new(1);

pub(super) struct Workers {
  events: EventSink,
  tasks: BTreeMap<String, Worker>,
  calls: HashSet<String>,
  owners: BTreeMap<String, String>,
}

struct Worker {
  id: String,
  name: String,
  task: Option<String>,
  run: Option<String>,
  status: WorkerStatus,
  detail: Option<String>,
}

impl Workers {
  pub fn new(events: EventSink) -> Self {
    Self {
      events,
      tasks: BTreeMap::new(),
      calls: HashSet::new(),
      owners: BTreeMap::new(),
    }
  }

  pub fn observe_calls(&mut self, value: &Value) {
    for block in blocks(value) {
      if field(block, "type") == Some("tool_use")
        && matches!(field(block, "name"), Some("Agent" | "Task"))
        && let Some(id) = field(block, "id")
      {
        self.calls.insert(id.to_owned());
      }
    }
  }

  pub fn sink_for(&self, value: &Value) -> Option<EventSink> {
    let Some(parent) = field(value, "parent_tool_use_id") else {
      return Some(self.events.clone());
    };
    let task = self.owners.get(parent)?;
    let session_id = self.tasks.get(task)?.id.clone();
    let events = self.events.clone();
    Some(EventSink::new(move |event| {
      events.emit(Event::SubagentEvent {
        session_id: session_id.clone(),
        event: Box::new(event),
      });
    }))
  }

  pub fn task(&mut self, value: &Value) {
    if value["ambient"] == true {
      return;
    }
    let Some(task) = field(value, "task_id") else {
      return;
    };
    let call = field(value, "tool_use_id");
    let recognized = match field(value, "task_type") {
      Some("local_agent" | "remote_agent") => true,
      Some(_) => false,
      None => call.is_some_and(|id| self.calls.contains(id)),
    };
    if !recognized && !self.tasks.contains_key(task) {
      return;
    }
    let subtype = field(value, "subtype");
    let mut description = field(value, "description");
    let (status, detail) = match subtype {
      Some("task_started") => (WorkerStatus::Running, None),
      Some("task_progress") => (WorkerStatus::Running, field(value, "summary")),
      Some("task_notification") => {
        let Some(status) = field(value, "status").and_then(status) else {
          return;
        };
        (status, field(value, "summary"))
      }
      Some("task_updated") => {
        let patch = &value["patch"];
        description = field(patch, "description").or(description);
        let Some(status) = field(patch, "status").and_then(status) else {
          return;
        };
        (status, field(patch, "error"))
      }
      _ => return,
    };
    let run = field(value, "run_id");
    if let Some(worker) = self.tasks.get(task) {
      if let (Some(seen), Some(run)) = (worker.run.as_deref(), run)
        && run < seen
      {
        return;
      }
      // Ignore a late progress update for an already finished run.
      let starts_new_run = match run {
        Some(run) => Some(run) != worker.run.as_deref(),
        None => subtype == Some("task_started"),
      };
      if !starts_new_run
        && status == WorkerStatus::Running
        && worker.status != WorkerStatus::Running
      {
        return;
      }
    }
    if let Some(call) = call {
      self.owners.insert(call.into(), task.into());
    }
    self.update(task, run, description, status, detail);
  }

  pub fn collaboration(&mut self, item: &Value) {
    let task = field(item, "prompt");
    let states = &item["agents_states"];
    if field(item, "tool") == Some("spawn_agent")
      && field(item, "status") == Some("completed")
      && let Some(ids) = item["receiver_thread_ids"].as_array()
    {
      for id in ids.iter().filter_map(Value::as_str) {
        if states.get(id).is_none() && !self.tasks.contains_key(id) {
          self.update(id, None, task, WorkerStatus::Running, None);
        }
      }
    }
    if let Some(states) = states.as_object() {
      for (id, state) in states {
        if let Some(status) = field(state, "status").and_then(status) {
          self.update(id, None, task, status, field(state, "message"));
        }
      }
    }
  }

  fn update(
    &mut self,
    key: &str,
    run: Option<&str>,
    task: Option<&str>,
    status: WorkerStatus,
    detail: Option<&str>,
  ) {
    let worker = self.tasks.entry(key.into()).or_insert_with(|| {
      let number = NEXT_WORKER.fetch_add(1, Ordering::Relaxed);
      Worker {
        id: Uuid::now_v7().to_string(),
        name: format!("worker {number}"),
        task: None,
        run: None,
        status: WorkerStatus::Unconfirmed,
        detail: None,
      }
    });
    let newer = run.is_some() && run != worker.run.as_deref();
    if let Some(run) = run {
      worker.run = Some(run.into());
    }
    let task = task
      .map(|text| truncate(text.to_owned(), 512))
      .or_else(|| worker.task.clone());
    let detail = detail.map(|text| truncate(text.to_owned(), 1200));
    if worker.status == status && worker.task == task && worker.detail == detail && !newer {
      return;
    }
    worker.status = status;
    worker.task = task;
    worker.detail = detail;
    worker.emit(&self.events);
  }
}

impl Worker {
  fn emit(&self, events: &EventSink) {
    events.emit(Event::WorkerUpdate {
      id: self.id.clone(),
      name: self.name.clone(),
      task: self.task.clone(),
      status: self.status,
      detail: self.detail.clone(),
    });
  }
}

impl Drop for Workers {
  fn drop(&mut self) {
    for worker in self
      .tasks
      .values_mut()
      .filter(|worker| worker.status == WorkerStatus::Running)
    {
      worker.status = WorkerStatus::Unconfirmed;
      worker.detail =
        Some("The provider stream closed before reporting a final worker status.".into());
      worker.emit(&self.events);
    }
  }
}

fn status(value: &str) -> Option<WorkerStatus> {
  Some(match value {
    "pending" | "pending_init" | "running" => WorkerStatus::Running,
    "completed" => WorkerStatus::Completed,
    "failed" | "errored" | "not_found" => WorkerStatus::Failed,
    "stopped" | "killed" | "interrupted" | "shutdown" => WorkerStatus::Stopped,
    _ => return None,
  })
}
