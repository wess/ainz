use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  process::{Child, ChildStdin, ChildStdout, Command},
};

use super::RpcResponse;
use crate::{
  mcp::McpServerConfig,
  process::{GroupGuard, kill_tree},
};

const MAX_LINE: u64 = 16 * 1024 * 1024;

pub(super) struct StdioTransport {
  // a server started through npx or a shell is a tree, and dropping the transport takes all
  // of it rather than only the process that was spawned
  _group: GroupGuard,
  pid: Option<u32>,
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
      .kill_on_drop(true)
      .process_group(0);
    if let Some(cwd) = &config.cwd {
      command.current_dir(cwd);
    }
    let mut child = command
      .spawn()
      .with_context(|| format!("start server {name}"))?;
    let pid = child.id();
    let stdin = child.stdin.take().context("server stdin unavailable")?;
    let stdout = BufReader::new(child.stdout.take().context("server stdout unavailable")?);
    Ok(Self {
      _group: GroupGuard::new(pid),
      pid,
      child,
      stdin,
      stdout,
    })
  }

  pub fn pid(&self) -> Option<u32> {
    self.pid
  }

  // a server that stopped answering is ended now, not whenever the hub next needs it
  pub fn stop(&self) {
    if let Some(pid) = self.pid {
      kill_tree(pid);
    }
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
      // a server waiting on its own request would otherwise wait for ever. ping is the one
      // thing a client offers without a capability, so it gets a real answer
      if let (Some(request), Some(method)) = (&response.id, &response.method) {
        let answer = match method.as_str() {
          "ping" => json!({"jsonrpc": "2.0", "id": request, "result": {}}),
          _ => json!({
            "jsonrpc": "2.0", "id": request,
            "error": {"code": -32601, "message": "client method not supported"}
          }),
        };
        self.write(&answer).await?;
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
