use std::{
  path::{Path, PathBuf},
  process::Stdio,
  time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
  io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
  process::Command,
  time::timeout,
};

use super::{PluginManifest, PluginTool};
use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

pub(super) struct ProcessTool {
  plugin: PluginManifest,
  root: PathBuf,
  definition: PluginTool,
}

impl ProcessTool {
  pub(super) fn new(plugin: PluginManifest, root: PathBuf, definition: PluginTool) -> Self {
    Self {
      plugin,
      root,
      definition,
    }
  }
}

#[async_trait]
impl Tool for ProcessTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: format!("{}_{}", self.plugin.plugin.name, self.definition.name),
      description: self.definition.description.clone(),
      parameters: serde_json::to_value(&self.definition.parameters)
        .unwrap_or_else(|_| json!({"type": "object"})),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    self
      .definition
      .capabilities
      .iter()
      .map(|capability| capability.risk())
      .max()
      .unwrap_or(Risk::Read)
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let (program, args) = self
      .plugin
      .runtime
      .command
      .split_first()
      .context("empty plugin command")?;
    let program = if Path::new(program).is_relative() {
      self.root.join(program)
    } else {
      PathBuf::from(program)
    };
    let mut child = Command::new(program)
      .args(args)
      .current_dir(&context.workspace)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .spawn()
      .context("start plugin")?;
    let request = json!({
      "version": 1, "id": uuid::Uuid::now_v7(), "method": "tool.call",
      "params": {"name": self.definition.name, "arguments": arguments,
      "context": {"workspace": context.workspace}}
    });
    let mut stdin = child.stdin.take().context("plugin stdin unavailable")?;
    stdin
      .write_all(serde_json::to_string(&request)?.as_bytes())
      .await?;
    stdin.write_all(b"\n").await?;
    drop(stdin);

    let stdout = child.stdout.take().context("plugin stdout unavailable")?;
    let stderr = child.stderr.take().context("plugin stderr unavailable")?;
    let capture_limit = context.max_output_bytes.saturating_add(16 * 1024);
    let run = async {
      let (status, stdout, stderr) = tokio::try_join!(
        child.wait(),
        capture(stdout, capture_limit),
        capture(stderr, capture_limit)
      )?;
      if stdout.truncated {
        bail!("plugin response exceeded {capture_limit} bytes");
      }
      if !status.success() {
        let detail = String::from_utf8_lossy(&stderr.bytes);
        bail!("plugin exited with {status}: {detail}");
      }
      Result::<Vec<u8>>::Ok(stdout.bytes)
    };
    let stdout = timeout(Duration::from_millis(self.plugin.runtime.timeout_ms), run)
      .await
      .context("plugin timed out")??;
    let line = std::str::from_utf8(&stdout).context("plugin response was not UTF-8")?;
    if line.lines().count() != 1 {
      bail!("plugin response must contain exactly one line");
    }
    let response: PluginResponse = serde_json::from_str(line).context("invalid plugin response")?;
    if let Some(error) = response.error {
      bail!("plugin error: {error}");
    }
    let output = match response.result.unwrap_or(Value::Null) {
      Value::String(output) => output,
      output => output.to_string(),
    };
    Ok(truncate(output, context.max_output_bytes))
  }
}

struct Capture {
  bytes: Vec<u8>,
  truncated: bool,
}

async fn capture(mut reader: impl AsyncRead + Unpin, limit: usize) -> std::io::Result<Capture> {
  let mut bytes = Vec::with_capacity(limit.min(8192));
  let mut buffer = [0_u8; 8192];
  let mut truncated = false;
  loop {
    let read = reader.read(&mut buffer).await?;
    if read == 0 {
      break;
    }
    let remaining = limit.saturating_sub(bytes.len());
    bytes.extend_from_slice(&buffer[..read.min(remaining)]);
    truncated |= read > remaining;
  }
  Ok(Capture { bytes, truncated })
}

#[derive(Deserialize)]
struct PluginResponse {
  result: Option<Value>,
  error: Option<String>,
}
