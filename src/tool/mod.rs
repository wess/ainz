mod builtin;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::ToolSpec;
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

#[derive(Clone, Debug)]
pub struct ToolContext {
  pub workspace: PathBuf,
  pub session_id: Uuid,
  pub max_output_bytes: usize,
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
