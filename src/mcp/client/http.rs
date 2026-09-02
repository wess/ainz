use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{
  Client, Response, StatusCode,
  header::{self, HeaderName, HeaderValue},
  redirect::Policy,
};
use serde_json::Value;

use super::{PROTOCOL_VERSION, RpcResponse};
use crate::{mcp::McpServerConfig, sse::SseDecoder};

const MAX_BODY: usize = 8 * 1024 * 1024;
const MAX_ERROR_BODY: usize = 2 * 1024;

pub(super) struct HttpTransport {
  client: Client,
  url: String,
  headers: Vec<(HeaderName, HeaderValue)>,
  session_id: Option<HeaderValue>,
  pub initialized: bool,
}

impl HttpTransport {
  pub fn new(config: &McpServerConfig) -> Result<Self> {
    let url = config
      .url
      .clone()
      .context("streamable_http server url is required")?;
    reqwest::Url::parse(&url).context("invalid streamable HTTP server URL")?;
    let mut headers = Vec::new();
    for (name, value) in &config.headers {
      headers.push(header_pair(name, value).with_context(|| format!("HTTP header {name}"))?);
    }
    for (name, variable) in &config.header_env {
      let value = std::env::var(variable)
        .with_context(|| format!("environment variable {variable} is required"))?;
      headers.push(header_pair(name, &value).with_context(|| format!("HTTP header {name}"))?);
    }
    // NB: redirects are refused so configured credentials never replay to another origin
    let client = Client::builder()
      .redirect(Policy::none())
      .connect_timeout(Duration::from_secs(15))
      .build()
      .context("build HTTP client")?;
    Ok(Self {
      client,
      url,
      headers,
      session_id: None,
      initialized: false,
    })
  }

  pub async fn exchange(
    &mut self,
    message: &Value,
    id: Option<u64>,
  ) -> Result<Option<RpcResponse>> {
    let mut request = self
      .client
      .post(&self.url)
      .header(header::ACCEPT, "application/json, text/event-stream")
      .header(header::CONTENT_TYPE, "application/json");
    if self.initialized {
      request = request.header("MCP-Protocol-Version", PROTOCOL_VERSION);
    }
    if let Some(session_id) = &self.session_id {
      request = request.header("MCP-Session-Id", session_id.clone());
    }
    for (name, value) in &self.headers {
      request = request.header(name.clone(), value.clone());
    }
    let response = request
      .json(message)
      .send()
      .await
      .context("send server request")?;
    if let Some(session_id) = response.headers().get("MCP-Session-Id") {
      self.session_id = Some(session_id.clone());
    }
    let status = response.status();
    if status == StatusCode::ACCEPTED {
      return Ok(None);
    }
    if !status.is_success() {
      let body = read_body(response, MAX_ERROR_BODY)
        .await
        .unwrap_or_default();
      bail!("HTTP server returned {status}: {}", body.trim());
    }
    let Some(id) = id else {
      return Ok(None);
    };
    let streaming = response
      .headers()
      .get(header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .is_some_and(|value| value.contains("text/event-stream"));
    if streaming {
      return read_stream(response, id).await.map(Some);
    }
    let body = read_body(response, MAX_BODY).await?;
    let value: Value = serde_json::from_str(&body).context("invalid server response")?;
    find_response(value, id)?
      .with_context(|| format!("server response did not answer request {id}"))
      .map(Some)
  }
}

fn header_pair(name: &str, value: &str) -> Result<(HeaderName, HeaderValue)> {
  Ok((
    HeaderName::from_bytes(name.as_bytes()).context("invalid header name")?,
    HeaderValue::from_str(value).context("invalid header value")?,
  ))
}

async fn read_body(response: Response, limit: usize) -> Result<String> {
  let mut stream = response.bytes_stream();
  let mut body = Vec::new();
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.context("read server response")?;
    if body.len() + chunk.len() > limit {
      bail!("server response exceeds {limit} bytes");
    }
    body.extend_from_slice(&chunk);
  }
  Ok(String::from_utf8_lossy(&body).into_owned())
}

// returns as soon as the answer arrives; a server may keep the stream open for progress
async fn read_stream(response: Response, id: u64) -> Result<RpcResponse> {
  let mut stream = response.bytes_stream();
  let mut decoder = SseDecoder::default();
  let mut total = 0;
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.context("read server stream")?;
    total += chunk.len();
    if total > MAX_BODY {
      bail!("server stream exceeds {MAX_BODY} bytes");
    }
    for data in decoder.push(&chunk) {
      if let Some(response) = parse_event(&data, id)? {
        return Ok(response);
      }
    }
  }
  for data in decoder.finish() {
    if let Some(response) = parse_event(&data, id)? {
      return Ok(response);
    }
  }
  bail!("server stream ended without answering request {id}")
}

fn parse_event(data: &str, id: u64) -> Result<Option<RpcResponse>> {
  let value: Value = serde_json::from_str(data).context("invalid server event")?;
  find_response(value, id)
}

fn find_response(value: Value, id: u64) -> Result<Option<RpcResponse>> {
  let items = match value {
    Value::Array(items) => items,
    value => vec![value],
  };
  for item in items {
    let response: RpcResponse = serde_json::from_value(item).context("invalid server message")?;
    if response.answers(id) {
      return Ok(Some(response));
    }
  }
  Ok(None)
}
