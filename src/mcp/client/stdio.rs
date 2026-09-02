use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
  io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
  process::{Child, ChildStdin, ChildStdout, Command},
  time::timeout,
};

use super::{PROTOCOL_VERSION, RpcResponse, ToolPage, call_output};
use crate::mcp::{McpServerConfig, RemoteTool};

pub(in crate::mcp) struct StdioClient {
  _child: Child,
  stdin: ChildStdin,
  stdout: BufReader<ChildStdout>,
  next_id: u64,
  timeout: Duration,
  instructions: Option<String>,
}

impl StdioClient {
  pub async fn start(name: &str, config: &McpServerConfig) -> Result<Self> {
    let (program, args) = config
      .command
      .split_first()
      .context("stdio server command cannot be empty")?;
    let mut command = Command::new(program);
    command
      .args(args)
      .envs(&config.env)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .kill_on_drop(true);
    if let Some(cwd) = &config.cwd {
      command.current_dir(cwd);
    }
    let mut child = command
      .spawn()
      .with_context(|| format!("start server {name}"))?;
    let stdin = child.stdin.take().context("server stdin unavailable")?;
    let stdout = BufReader::new(child.stdout.take().context("server stdout unavailable")?);
    let mut client = Self {
      _child: child,
      stdin,
      stdout,
      next_id: 1,
      timeout: Duration::from_millis(config.timeout_ms),
      instructions: None,
    };
    client.initialize(name).await?;
    Ok(client)
  }

  async fn initialize(&mut self, name: &str) -> Result<()> {
    let result = self
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
    self.instructions = result
      .get("instructions")
      .and_then(Value::as_str)
      .map(str::to_owned);
    self.notify("notifications/initialized", json!({})).await
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
    self
      .write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
      .await?;
    timeout(self.timeout, async {
      loop {
        let mut line = String::new();
        if self.stdout.read_line(&mut line).await? == 0 {
          bail!("server closed stdout")
        }
        let response: RpcResponse = serde_json::from_str(line.trim())?;
        if response.id == Some(json!(id)) {
          if let Some(error) = response.error {
            bail!("server error {}: {}", error.code, error.message)
          }
          return response.result.context("server response had no result");
        }
        if response.id.is_some() && response.method.is_some() {
          self
            .write(json!({
              "jsonrpc": "2.0", "id": response.id,
              "error": {"code": -32601, "message": "client method not supported"}
            }))
            .await?;
        }
      }
    })
    .await
    .context("server request timed out")?
  }

  async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
    self
      .write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
      .await
  }

  async fn write(&mut self, value: Value) -> Result<()> {
    self
      .stdin
      .write_all(serde_json::to_string(&value)?.as_bytes())
      .await?;
    self.stdin.write_all(b"\n").await?;
    self.stdin.flush().await?;
    Ok(())
  }
}
