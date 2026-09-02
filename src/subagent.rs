use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
  protocol::{ToolSpec, Usage},
  tool::{Risk, Tool, ToolContext, truncate},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubagentResult {
  pub session_id: Uuid,
  pub output: String,
  pub usage: Usage,
}

pub type SubagentFuture = Pin<Box<dyn Future<Output = Result<SubagentResult>> + Send>>;
pub type SubagentHandler = Arc<dyn Fn(Uuid, String) -> SubagentFuture + Send + Sync>;

pub fn subagent_tool(handler: SubagentHandler) -> Arc<dyn Tool> {
  Arc::new(SubagentTool { handler })
}

struct SubagentTool {
  handler: SubagentHandler,
}

#[async_trait]
impl Tool for SubagentTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "subagent".into(),
      description: "Delegate a focused task to a durable child session".into(),
      parameters: json!({
        "type": "object", "properties": {
          "prompt": {"type": "string", "minLength": 1}
        }, "required": ["prompt"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Execute
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let prompt = arguments
      .get("prompt")
      .and_then(Value::as_str)
      .context("prompt is required")?;
    let result = (self.handler)(context.session_id, prompt.into()).await?;
    Ok(truncate(
      serde_json::to_string_pretty(&result)?,
      context.max_output_bytes,
    ))
  }
}
