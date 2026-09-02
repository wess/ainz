use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  process::{Child, ChildStdin, ChildStdout, Command},
};

use super::RpcResponse;
use crate::mcp::McpServerConfig;

const MAX_LINE: u64 = 16 * 1024 * 1024;

pub(super) struct StdioTransport {
  child: Child,
  stdin: ChildStdin,
  stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
  pub fn start(name: &str, config: &McpServerConfig) -> Result<Self> {
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
    Ok(Self {
      child,
      stdin,
      stdout,
    })
  }

  // a server that quit between requests is noticed before anything is sent to it
  pub fn alive(&mut self) -> bool {
    self.child.try_wait().is_ok_and(|status| status.is_none())
  }

  pub async fn exchange(
    &mut self,
    message: &Value,
    id: Option<u64>,
  ) -> Result<Option<RpcResponse>> {
    self.write(message).await?;
    let Some(id) = id else {
      return Ok(None);
    };
    loop {
      let mut line = String::new();
      let read = (&mut self.stdout)
        .take(MAX_LINE)
        .read_line(&mut line)
        .await
        .context("read server stdout")?;
      if read == 0 {
        bail!("server closed stdout");
      }
      if !line.ends_with('\n') && u64::try_from(read).is_ok_and(|read| read >= MAX_LINE) {
        bail!("server sent a line over {MAX_LINE} bytes");
      }
      // servers launched through package runners often print banners on stdout first
      let Ok(response) = serde_json::from_str::<RpcResponse>(line.trim()) else {
        continue;
      };
      if response.answers(id) {
        return Ok(Some(response));
      }
      if response.id.is_some() && response.method.is_some() {
        self
          .write(&json!({
            "jsonrpc": "2.0", "id": response.id,
            "error": {"code": -32601, "message": "client method not supported"}
          }))
          .await?;
      }
    }
  }

  async fn write(&mut self, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    self
      .stdin
      .write_all(&bytes)
      .await
      .context("write to server stdin")?;
    self.stdin.flush().await.context("flush server stdin")
  }
}
