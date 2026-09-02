use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

use super::McpHub;

pub(super) struct McpTool {
  hub: Arc<McpHub>,
}

impl McpTool {
  pub fn new(hub: Arc<McpHub>) -> Self {
    Self { hub }
  }

  async fn search(&self, query: &str) -> Result<String> {
    let query = query.to_lowercase();
    let mut matches = Vec::new();
    for (server, tools) in self.hub.searchable_tools().await? {
      for tool in tools.iter() {
        if query.is_empty()
          || tool.name.to_lowercase().contains(&query)
          || tool.description.to_lowercase().contains(&query)
        {
          matches.push(format!("{server}/{}: {}", tool.name, tool.description));
        }
      }
    }
    Ok(matches.join("\n"))
  }
}

#[async_trait]
impl Tool for McpTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "mcp".into(),
      description: format!(
        "Discover and call external tools without loading every schema. Servers: {}",
        self.hub.server_names().join(", ")
      ),
      parameters: json!({
        "type": "object", "properties": {
          "command": {"type": "string", "enum": ["search", "schema", "call"]},
          "query": {"type": "string"}, "server": {"type": "string"},
          "name": {"type": "string"}, "arguments": {"type": "object"}
        }, "required": ["command"], "additionalProperties": false
      }),
    }
  }

  // the server's own readOnlyHint/destructiveHint annotations decide; nothing is trusted by name
  fn risk(&self, arguments: &Value) -> Risk {
    if arguments.get("command").and_then(Value::as_str) != Some("call") {
      return Risk::Read;
    }
    match target(arguments) {
      Ok((server, name)) => self.hub.cached_risk(server, name),
      Err(_) => Risk::Execute,
    }
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let command = arguments
      .get("command")
      .and_then(Value::as_str)
      .context("command is required")?;
    let output = match command {
      "search" => {
        self
          .search(arguments.get("query").and_then(Value::as_str).unwrap_or(""))
          .await?
      }
      "schema" => {
        let (server, name) = target(&arguments)?;
        let tools = self.hub.tools(server).await?;
        let tool = tools
          .iter()
          .find(|tool| tool.name == name)
          .with_context(|| format!("tool {server}/{name} was not found"))?;
        serde_json::to_string_pretty(&json!({
          "server": server, "name": tool.name, "description": tool.description,
          "input_schema": tool.input_schema,
        }))?
      }
      "call" => {
        let (server, name) = target(&arguments)?;
        self
          .hub
          .call(
            server,
            name,
            arguments.get("arguments").cloned().unwrap_or(json!({})),
          )
          .await?
      }
      _ => bail!("unknown mcp command: {command}"),
    };
    Ok(truncate(output, context.max_output_bytes))
  }
}

fn target(arguments: &Value) -> Result<(&str, &str)> {
  let server = arguments
    .get("server")
    .and_then(Value::as_str)
    .context("server is required")?;
  let name = arguments
    .get("name")
    .and_then(Value::as_str)
    .context("name is required")?;
  Ok((server, name))
}
