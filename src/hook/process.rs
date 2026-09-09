use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

use crate::{config::HookDef, output::capture, process::GroupGuard};

// a hook is someone else's script, not part of ainz's own control flow, so one that hangs
// (waits on input, loops, calls out to something slow) must not be able to hang the session
const MAX_HOOK_OUTPUT: usize = 64 * 1024;
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct HookOutcome {
  pub success: bool,
  pub stderr: String,
}

pub(super) async fn run_hook(
  def: &HookDef,
  payload: &Value,
  workspace: &Path,
) -> Result<HookOutcome> {
  let Some((program, args)) = def.command.split_first() else {
    bail!("hook command is empty");
  };
  let mut child = Command::new(program)
    .current_dir(workspace)
    .args(args)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .process_group(0)
    .spawn()
    .with_context(|| format!("start hook {program}"))?;
  let guard = GroupGuard::new(child.id());
  let mut stdin = child.stdin.take().expect("stdin was piped");
  let bytes = serde_json::to_vec(payload).context("encode hook payload")?;
  let stdout = child.stdout.take().expect("stdout was piped");
  let stderr = child.stderr.take().expect("stderr was piped");
  let write = async {
    stdin.write_all(&bytes).await.context("write hook stdin")?;
    drop(stdin);
    Ok::<_, anyhow::Error>(())
  };
  let run = async {
    let (_, _, stderr, status) = tokio::try_join!(
      write,
      async { capture(stdout, 0).await.context("drain hook stdout") },
      async {
        capture(stderr, MAX_HOOK_OUTPUT)
          .await
          .context("read hook stderr")
      },
      async { child.wait().await.context("wait for hook") },
    )?;
    let mut text = String::from_utf8_lossy(&stderr.bytes).into_owned();
    if stderr.truncated {
      text.push_str("\n[hook output truncated]");
    }
    Ok::<_, anyhow::Error>(HookOutcome {
      success: status.success(),
      stderr: text,
    })
  };
  let output = timeout(HOOK_TIMEOUT, run)
    .await
    .with_context(|| format!("hook {program} timed out after {HOOK_TIMEOUT:?}"))??;
  guard.disarm();
  Ok(output)
}
