use std::{
  path::{Path, PathBuf},
  process::Stdio,
  sync::Arc,
  time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

use super::{PluginManifest, PluginTool, capture, catalog::file_digest};
use crate::{
  process::GroupGuard,
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

pub(super) struct ProcessTool {
  plugin: Arc<PluginManifest>,
  program: PathBuf,
  digest: String,
  definition: PluginTool,
}

impl ProcessTool {
  pub(super) fn new(
    plugin: Arc<PluginManifest>,
    root: &Path,
    digest: String,
    definition: PluginTool,
  ) -> Result<Self> {
    let program = plugin
      .runtime
      .command
      .first()
      .context("empty plugin command")?;
    let program = if Path::new(program).is_relative() {
      root.join(program)
    } else {
      PathBuf::from(program)
    };
    Ok(Self {
      plugin,
      program,
      digest,
      definition,
    })
  }
}

#[async_trait]
impl Tool for ProcessTool {
  fn spec(&self) -> ToolSpec {
    self.definition.spec(&self.plugin.plugin.name)
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    self.definition.risk()
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    // the approval pinned this program's content; a swapped binary does not inherit it
    if file_digest(&self.program).await? != self.digest {
      bail!(
        "{} changed since the plugin was approved",
        self.program.display()
      );
    }
    let mut child = Command::new(&self.program)
      .args(&self.plugin.runtime.command[1..])
      .current_dir(&context.workspace)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .process_group(0)
      .spawn()
      .context("start plugin")?;
    let guard = GroupGuard::new(child.id());
    let request = json!({
      "version": 1, "id": uuid::Uuid::now_v7(), "method": "tool.call",
      "params": {"name": self.definition.name, "arguments": arguments,
      "context": {"workspace": context.workspace}}
    });
    let mut stdin = child.stdin.take().context("plugin stdin unavailable")?;
    let stdout = child.stdout.take().context("plugin stdout unavailable")?;
    let stderr = child.stderr.take().context("plugin stderr unavailable")?;
    let capture_limit = context.max_output_bytes.saturating_add(16 * 1024);
    let mut request = serde_json::to_vec(&request)?;
    request.push(b'\n');
    // the request is fed while output drains; a child that quits early explains itself on stderr
    let feed = async move {
      drop(stdin.write_all(&request).await);
      drop(stdin.shutdown().await);
    };
    let run = async {
      let ((), status, stdout, stderr) = tokio::join!(
        feed,
        child.wait(),
        capture(stdout, capture_limit),
        capture(stderr, capture_limit)
      );
      let (status, stdout, stderr) = (status?, stdout?, stderr?);
      if stdout.truncated {
        bail!("plugin response exceeded {capture_limit} bytes");
      }
      if !status.success() {
        let detail = String::from_utf8_lossy(&stderr.bytes);
        bail!("plugin exited with {status}: {}", detail.trim());
      }
      Result::<Vec<u8>>::Ok(stdout.bytes)
    };
    let stdout = timeout(Duration::from_millis(self.plugin.runtime.timeout_ms), run)
      .await
      .context("plugin timed out")??;
    guard.disarm();
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

#[derive(Deserialize)]
struct PluginResponse {
  result: Option<Value>,
  error: Option<String>,
}
