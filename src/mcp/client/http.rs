use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header};
use serde_json::{Value, json};

use super::{PROTOCOL_VERSION, RpcResponse, ToolPage, call_output};
use crate::mcp::{McpServerConfig, RemoteTool};

pub(in crate::mcp) struct HttpClient {
  client: Client,
  url: String,
  headers: BTreeMap<String, String>,
  session_id: Option<String>,
  initialized: bool,
  next_id: u64,
  timeout: Duration,
  instructions: Option<String>,
}

impl HttpClient {
  pub async fn start(name: &str, config: &McpServerConfig) -> Result<Self> {
    let url = config
      .url
      .clone()
      .context("streamable_http server url is required")?;
    reqwest::Url::parse(&url).context("invalid streamable HTTP server URL")?;
    let mut headers = BTreeMap::new();
    for (header, value) in &config.headers {
      header::HeaderName::from_bytes(header.as_bytes())
        .with_context(|| format!("invalid HTTP header name {header}"))?;
      header::HeaderValue::from_str(value)
        .with_context(|| format!("invalid value for HTTP header {header}"))?;
      headers.insert(header.clone(), value.clone());
    }
    for (header, variable) in &config.header_env {
      let value = std::env::var(variable)
        .with_context(|| format!("environment variable {variable} is required"))?;
      header::HeaderName::from_bytes(header.as_bytes())
        .with_context(|| format!("invalid HTTP header name {header}"))?;
      header::HeaderValue::from_str(&value)
        .with_context(|| format!("invalid value from {variable}"))?;
      headers.insert(header.clone(), value);
    }
    let mut client = Self {
      client: Client::new(),
      url,
      headers,
      session_id: None,
      initialized: false,
      next_id: 1,
      timeout: Duration::from_millis(config.timeout_ms),
      instructions: None,
    };
    let result = client
      .request(
        "initialize",
        json!({
          "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
          "clientInfo": {"name": "agentx", "version": env!("CARGO_PKG_VERSION")}
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
    client.initialized = true;
    client
      .notify("notifications/initialized", json!({}))
      .await?;
    Ok(client)
  }

  pub fn instructions(&self) -> Option<&str> {
    self.instructions.as_deref()
  }

  pub async fn list_tools(&mut self) -> Result<Vec<RemoteTool>> {
    let mut tools = Vec::new();
    let mut cursor = None;
    loop {
      let result = self
        .request(
          "tools/list",
          cursor
            .as_ref()
            .map_or(json!({}), |cursor| json!({"cursor": cursor})),
        )
        .await?;
      let page: ToolPage = serde_json::from_value(result)?;
      tools.extend(page.tools);
      cursor = page.next_cursor;
      if cursor.is_none() {
        break;
      }
    }
    Ok(tools)
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
    let value = self
      .post(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
      .await?
      .context("server returned no response")?;
    response_result(value, id)
  }

  async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
    self
      .post(json!({"jsonrpc": "2.0", "method": method, "params": params}))
      .await?;
    Ok(())
  }

  async fn post(&mut self, value: Value) -> Result<Option<Value>> {
    let mut request = self
      .client
      .post(&self.url)
      .timeout(self.timeout)
      .header(header::ACCEPT, "application/json, text/event-stream")
      .header(header::CONTENT_TYPE, "application/json");
    if self.initialized {
      request = request.header("MCP-Protocol-Version", PROTOCOL_VERSION);
    }
    if let Some(session_id) = &self.session_id {
      request = request.header("MCP-Session-Id", session_id);
    }
    for (name, value) in &self.headers {
      request = request.header(name, value);
    }
    let response = request.json(&value).send().await?;
    if let Some(session_id) = response
      .headers()
      .get("MCP-Session-Id")
      .and_then(|value| value.to_str().ok())
    {
      self.session_id = Some(session_id.into());
    }
    if response.status() == StatusCode::ACCEPTED {
      return Ok(None);
    }
    let status = response.status();
    if !status.is_success() {
      bail!("HTTP server returned {status}: {}", response.text().await?)
    }
    let streaming = response
      .headers()
      .get(header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .is_some_and(|value| value.contains("text/event-stream"));
    if streaming {
      parse_sse(&response.text().await?).map(Some)
    } else {
      Ok(Some(response.json().await?))
    }
  }
}

fn response_result(value: Value, id: u64) -> Result<Value> {
  let mut values = Vec::new();
  flatten(value, &mut values);
  for value in values {
    let response: RpcResponse = serde_json::from_value(value)?;
    if response.id == Some(json!(id)) {
      if let Some(error) = response.error {
        bail!("server error {}: {}", error.code, error.message)
      }
      return response.result.context("server response had no result");
    }
  }
  bail!("server response did not contain request id {id}")
}

fn flatten(value: Value, values: &mut Vec<Value>) {
  match value {
    Value::Array(items) => {
      for item in items {
        flatten(item, values);
      }
    }
    value => values.push(value),
  }
}

fn parse_sse(body: &str) -> Result<Value> {
  let mut messages = Vec::new();
  for event in body.split("\n\n") {
    let data = event
      .lines()
      .filter_map(|line| line.strip_prefix("data:"))
      .map(str::trim)
      .collect::<Vec<_>>()
      .join("\n");
    if !data.is_empty() {
      messages.push(serde_json::from_str(&data).context("invalid server event")?);
    }
  }
  match messages.len() {
    0 => bail!("server event stream contained no data"),
    1 => Ok(messages.pop().unwrap()),
    _ => Ok(Value::Array(messages)),
  }
}
