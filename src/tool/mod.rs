mod builtin;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
  event::{Event, EventSink},
  protocol::ToolSpec,
};
use uuid::Uuid;

pub use builtin::builtins;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
  Read,
  Write,
  Execute,
  Network,
}

#[derive(Clone)]
pub struct ToolContext {
  pub workspace: PathBuf,
  pub session_id: Uuid,
  pub max_output_bytes: usize,
  // where a tool reports what it has produced so far, and the call it belongs to
  pub progress: Option<(EventSink, String)>,
}

impl ToolContext {
  pub fn new(workspace: PathBuf, session_id: Uuid, max_output_bytes: usize) -> Self {
    Self {
      workspace,
      session_id,
      max_output_bytes,
      progress: None,
    }
  }

  /// A line a tool has just produced. A run that says nothing for three minutes looks the
  /// same as one that has wedged, so anything long-running says so as it goes.
  pub fn report(&self, text: &str) {
    if let Some((events, id)) = &self.progress {
      events.emit(Event::ToolDelta {
        id: id.clone(),
        text: text.into(),
      });
    }
  }
}

#[async_trait]
pub trait Tool: Send + Sync {
  fn spec(&self) -> ToolSpec;
  fn risk(&self, arguments: &Value) -> Risk;
  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String>;
}

#[derive(Clone, Default)]
pub struct ToolSet {
  tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolSet {
  pub fn insert(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
    let name = tool.spec().name;
    if self.tools.insert(name.clone(), tool).is_some() {
      bail!("duplicate tool name: {name}");
    }
    Ok(())
  }

  // for a tool that is being rebound rather than added, such as a child's own server hub
  pub fn replace(&mut self, tool: Arc<dyn Tool>) {
    self.tools.insert(tool.spec().name, tool);
  }

  pub fn extend(&mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<()> {
    for tool in tools {
      self.insert(tool)?;
    }
    Ok(())
  }

  pub fn specs(&self) -> Vec<ToolSpec> {
    let mut specs: Vec<_> = self.tools.values().map(|tool| tool.spec()).collect();
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
  }

  pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
    self.tools.get(name)
  }
}

pub fn truncate(mut value: String, limit: usize) -> String {
  if value.len() <= limit {
    return value;
  }
  let mut end = limit;
  while !value.is_char_boundary(end) {
    end -= 1;
  }
  value.truncate(end);
  value.push_str("\n[output truncated]");
  value
}
