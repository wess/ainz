mod http;
mod stdio;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use super::{McpServerConfig, McpTransport, RemoteTool};

pub(super) const PROTOCOL_VERSION: &str = "2025-11-25";

pub(super) enum Client {
  Stdio(stdio::StdioClient),
  Http(http::HttpClient),
}

impl Client {
  pub async fn start(name: &str, config: &McpServerConfig) -> Result<Self> {
    match config.transport {
      McpTransport::Stdio => Ok(Self::Stdio(stdio::StdioClient::start(name, config).await?)),
      McpTransport::StreamableHttp => Ok(Self::Http(http::HttpClient::start(name, config).await?)),
    }
  }

  pub async fn list_tools(&mut self) -> Result<Vec<RemoteTool>> {
    match self {
      Self::Stdio(client) => client.list_tools().await,
      Self::Http(client) => client.list_tools().await,
    }
  }

  pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String> {
    match self {
      Self::Stdio(client) => client.call_tool(name, arguments).await,
      Self::Http(client) => client.call_tool(name, arguments).await,
    }
  }

  pub fn instructions(&self) -> Option<&str> {
    match self {
      Self::Stdio(client) => client.instructions(),
      Self::Http(client) => client.instructions(),
    }
  }
}

#[derive(Deserialize)]
pub(super) struct ToolPage {
  pub tools: Vec<RemoteTool>,
  #[serde(rename = "nextCursor")]
  pub next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct CallResult {
  #[serde(default)]
  content: Vec<ContentBlock>,
  #[serde(rename = "structuredContent")]
  structured_content: Option<Value>,
  #[serde(default, rename = "isError")]
  is_error: bool,
}

#[derive(Deserialize)]
struct ContentBlock {
  text: Option<String>,
}

pub(super) fn call_output(value: Value) -> Result<String> {
  let result: CallResult = serde_json::from_value(value)?;
  let mut output = result
    .content
    .into_iter()
    .filter_map(|block| block.text)
    .collect::<Vec<_>>()
    .join("\n");
  if let Some(structured) = result.structured_content {
    if !output.is_empty() {
      output.push('\n');
    }
    output.push_str(&structured.to_string());
  }
  if result.is_error {
    output.insert_str(0, "[tool error] ");
  }
  Ok(output)
}

#[derive(Deserialize)]
pub(super) struct RpcResponse {
  pub id: Option<Value>,
  pub result: Option<Value>,
  pub error: Option<RpcError>,
  pub method: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct RpcError {
  pub code: i64,
  pub message: String,
}
