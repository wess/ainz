//! A tool a script gave the model. It runs on a blocking thread, so a script that takes its time
//! holds up only the call it is answering, never the interface's event loop.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use mlua::{Function, LuaSerdeExt, Value};
use serde_json::Value as Json;

use super::Script;
use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

/// How long a script's tool may run. Longer than an interface callback: a tool is work.
const LIMIT: Duration = Duration::from_secs(120);

pub(super) struct LuaTool {
  script: Arc<Script>,
  name: String,
}

impl LuaTool {
  pub(super) fn new(script: Arc<Script>, name: String) -> Self {
    Self { script, name }
  }

  fn definition(&self) -> Option<(String, Json, Risk, Function)> {
    let registry = self
      .script
      .shared
      .registry
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tool = registry.tools.get(&self.name)?;
    Some((
      tool.description.clone(),
      tool.parameters.clone(),
      tool.risk,
      tool.function.clone(),
    ))
  }
}

#[async_trait]
impl Tool for LuaTool {
  fn spec(&self) -> ToolSpec {
    let (description, parameters) = match self.definition() {
      Some((description, parameters, ..)) => (description, parameters),
      None => (
        "no longer defined; /source dropped it".into(),
        serde_json::json!({"type": "object", "properties": {}}),
      ),
    };
    ToolSpec {
      name: self.name.clone(),
      description,
      parameters,
    }
  }

  fn risk(&self, _arguments: &Json) -> Risk {
    self
      .definition()
      .map_or(Risk::Execute, |(_, _, risk, _)| risk)
  }

  async fn execute(&self, context: &ToolContext, arguments: Json) -> Result<String> {
    let (.., function) = self
      .definition()
      .with_context(|| format!("{} is no longer defined; /source dropped it", self.name))?;
    let script = self.script.clone();
    let workspace = context.workspace.display().to_string();
    let output = tokio::task::spawn_blocking(move || -> Result<String> {
      let lua = &script.lua;
      let args = lua.to_value_with(&arguments, super::serialize())?;
      let call = lua.create_table()?;
      call.set("workspace", workspace)?;
      let result = script
        .guarded(LIMIT, || function.call::<Value>((args, call)))
        .map_err(|error| anyhow::anyhow!(super::short(&error)))?;
      Ok(match result {
        Value::Nil => String::new(),
        Value::String(text) => text.to_string_lossy(),
        other => serde_json::to_string(&lua.from_value::<Json>(other)?)?,
      })
    })
    .await
    .context("the script's tool stopped")??;
    Ok(truncate(output, context.max_output_bytes))
  }
}
