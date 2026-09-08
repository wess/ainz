use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command, sync::mpsc, time::timeout};

use super::{ToolContext, truncate};
use crate::process::GroupGuard;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ShellArgs {
  command: String,
  #[serde(default = "default_timeout")]
  timeout_ms: u64,
}

fn default_timeout() -> u64 {
  30_000
}

pub(super) async fn execute(context: &ToolContext, value: Value) -> Result<String> {
  let args: ShellArgs = serde_json::from_value(value)?;
  let timeout_ms = args.timeout_ms.clamp(100, 300_000);
  let mut child = Command::new("sh")
    .args(["-c", &args.command])
    .current_dir(&context.workspace)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .process_group(0)
    .spawn()
    .context("start shell")?;
  let guard = GroupGuard::new(child.id());
  let (sender, mut chunks) = mpsc::channel::<String>(8);
  // both pipes are read as they fill, which is what makes a long command visible while it
  // runs; the cost is that stderr lands where it happened rather than after all of stdout
  for pipe in [
    child.stdout.take().map(Pipe::Out),
    child.stderr.take().map(Pipe::Err),
  ]
  .into_iter()
  .flatten()
  {
    let sender = sender.clone();
    tokio::spawn(async move {
      match pipe {
        Pipe::Out(pipe) => forward(pipe, sender).await,
        Pipe::Err(pipe) => forward(pipe, sender).await,
      }
    });
  }
  drop(sender);
  let drain = async {
    let mut text = String::new();
    let mut truncated = false;
    while let Some(chunk) = chunks.recv().await {
      if truncated {
        continue;
      }
      let mut taken = chunk
        .len()
        .min(context.max_output_bytes.saturating_sub(text.len()));
      while !chunk.is_char_boundary(taken) {
        taken -= 1;
      }
      if taken > 0 {
        context.report(&chunk[..taken]);
        text.push_str(&chunk[..taken]);
      }
      if taken < chunk.len() && !truncated {
        context.report("\n[output truncated]\n");
        truncated = true;
      }
    }
    if truncated {
      text.push_str("\n[output truncated]");
    }
    text
  };
  let Ok((status, mut text)) = timeout(Duration::from_millis(timeout_ms), async {
    tokio::join!(child.wait(), drain)
  })
  .await
  else {
    bail!("command timed out after {timeout_ms} ms");
  };
  let status = status.context("wait for shell")?;
  guard.disarm();
  if !text.is_empty() && !text.ends_with('\n') {
    text.push('\n');
  }
  text.push_str(&format!("[exit {}]", status.code().unwrap_or(-1)));
  Ok(truncate(text, context.max_output_bytes))
}

enum Pipe {
  Out(tokio::process::ChildStdout),
  Err(tokio::process::ChildStderr),
}

async fn forward<R: tokio::io::AsyncRead + Unpin>(mut reader: R, sender: mpsc::Sender<String>) {
  let mut buffer = [0_u8; 8192];
  let mut pending = Vec::new();
  while let Ok(read) = reader.read(&mut buffer).await {
    if read == 0 {
      if !pending.is_empty() {
        let _ = sender
          .send(String::from_utf8_lossy(&pending).into_owned())
          .await;
      }
      break;
    }
    pending.extend_from_slice(&buffer[..read]);
    let mut text = String::new();
    let mut consumed = 0;
    while consumed < pending.len() {
      match std::str::from_utf8(&pending[consumed..]) {
        Ok(valid) => {
          text.push_str(valid);
          consumed = pending.len();
        }
        Err(error) => {
          let end = consumed + error.valid_up_to();
          text.push_str(
            std::str::from_utf8(&pending[consumed..end]).expect("validated UTF-8 prefix"),
          );
          consumed = end;
          let Some(length) = error.error_len() else {
            break;
          };
          text.push(char::REPLACEMENT_CHARACTER);
          consumed += length;
        }
      }
    }
    pending.drain(..consumed);
    if !text.is_empty() && sender.send(text).await.is_err() {
      break;
    }
  }
}
