mod http;
mod stdio;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::timeout;

use super::{McpServerConfig, McpTransport, RemoteTool};

pub(super) const PROTOCOL_VERSION: &str = "2025-11-25";

// one initialized session over either transport. the handshake, paging, and result shaping
// live here once; a transport only moves one message and finds its reply
pub(super) struct Client {
  transport: Transport,
  next_id: u64,
  timeout: Duration,
  broken: bool,
  instructions: Option<String>,
}

enum Transport {
  Stdio(stdio::StdioTransport),
  Http(http::HttpTransport),
}

impl Transport {
  async fn exchange(&mut self, message: &Value, id: Option<u64>) -> Result<Option<RpcResponse>> {
    match self {
      Self::Stdio(transport) => transport.exchange(message, id).await,
      Self::Http(transport) => transport.exchange(message, id).await,
    }
  }
}

impl Client {
  pub async fn start(name: &str, config: &McpServerConfig) -> Result<Self> {
    let transport = match config.transport {
      McpTransport::Stdio => Transport::Stdio(stdio::StdioTransport::start(name, config)?),
      McpTransport::StreamableHttp => Transport::Http(http::HttpTransport::new(config)?),
    };
    let mut client = Self {
      transport,
      next_id: 1,
      timeout: Duration::from_millis(config.timeout_ms),
      broken: false,
      instructions: None,
    };
    let result = client
      .request(
        "initialize",
        json!({
          "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
          "clientInfo": {"name": "ainz", "version": env!("CARGO_PKG_VERSION")}
        }),
      )
      .await?;
    let version = result
      .get("protocolVersion")
      .and_then(Value::as_str)
      .unwrap_or_default();
    if version != PROTOCOL_VERSION {
      bail!("server {name} negotiated unsupported version {version}");
    }
    client.instructions = result
      .get("instructions")
      .and_then(Value::as_str)
      .map(str::to_owned);
    if let Transport::Http(transport) = &mut client.transport {
      transport.initialized = true;
    }
    client
      .notify("notifications/initialized", json!({}))
      .await?;
    Ok(client)
  }

  // a transport failure or timeout leaves the stream in an unknown state, and a stdio server
  // may simply have exited; either way the hub drops the client and starts a fresh one
  pub fn broken(&mut self) -> bool {
    self.broken
      || match &mut self.transport {
        Transport::Stdio(transport) => !transport.alive(),
        Transport::Http(_) => false,
      }
  }

  pub fn instructions(&self) -> Option<&str> {
    self.instructions.as_deref()
  }

  pub async fn list_tools(&mut self) -> Result<Vec<RemoteTool>> {
    let mut tools = Vec::new();
    let mut cursor = None;
    loop {
      let params = cursor
        .as_ref()
        .map_or(json!({}), |cursor| json!({"cursor": cursor}));
      let page: ToolPage = serde_json::from_value(self.request("tools/list", params).await?)
        .context("invalid tools/list result")?;
      tools.extend(page.tools);
      cursor = page.next_cursor;
      if cursor.is_none() {
        return Ok(tools);
      }
    }
  }

  pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String> {
    call_output(
      self
        .request("tools/call", json!({"name": name, "arguments": arguments}))
        .await?,
    )
  }

  async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
    let id = self.next_id;
    self.next_id += 1;
    let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let response = match timeout(self.timeout, self.transport.exchange(&message, Some(id))).await {
      Ok(Ok(Some(response))) => response,
      Ok(Ok(None)) => bail!("server accepted {method} without answering"),
      Ok(Err(error)) => {
        self.broken = true;
        return Err(error);
      }
      Err(_) => {
        self.broken = true;
        bail!("server request timed out");
      }
    };
    if let Some(error) = response.error {
      bail!("server error {}: {}", error.code, error.message);
    }
    Ok(response.result.unwrap_or(Value::Null))
  }

  async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
    let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
    match timeout(self.timeout, self.transport.exchange(&message, None)).await {
      Ok(Ok(_)) => Ok(()),
      Ok(Err(error)) => {
        self.broken = true;
        Err(error)
      }
      Err(_) => {
        self.broken = true;
        bail!("server notification timed out")
      }
    }
  }
}

#[derive(Deserialize)]
struct ToolPage {
  tools: Vec<RemoteTool>,
  #[serde(rename = "nextCursor")]
  next_cursor: Option<String>,
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
  #[serde(default, rename = "type")]
  kind: String,
  text: Option<String>,
}

fn call_output(value: Value) -> Result<String> {
  let result: CallResult = serde_json::from_value(value).context("invalid tools/call result")?;
  let mut output = result
    .content
    .into_iter()
    .map(|block| {
      block
        .text
        .unwrap_or_else(|| format!("[{} content omitted]", block.kind))
    })
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

impl RpcResponse {
  fn answers(&self, id: u64) -> bool {
    self.id == Some(json!(id)) && self.method.is_none()
  }
}
