use std::{path::Path, process::Stdio, time::Duration};

// dependencies
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, time::timeout};

// project
use crate::{output::capture, process::GroupGuard};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CheckResult {
  pub command: String,
  pub code: Option<i32>,
  pub timed_out: bool,
  pub elapsed_ms: u64,
  pub stdout: String,
  pub stderr: String,
}

impl CheckResult {
  pub fn passed(&self) -> bool {
    !self.timed_out && self.code == Some(0)
  }
}

pub(super) async fn check(
  workspace: &Path,
  build: &Path,
  command: &str,
  seconds: u64,
) -> Result<CheckResult> {
  ensure!(
    !command.trim().is_empty(),
    "a check command cannot be empty"
  );
  ensure!(
    (1..=3600).contains(&seconds),
    "check timeout must be 1..=3600 seconds"
  );
  let started = std::time::Instant::now();
  let mut child = Command::new("sh")
    .args(["-c", command])
    .current_dir(workspace)
    .env("CARGO_TARGET_DIR", build)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .process_group(0)
    .kill_on_drop(true)
    .spawn()
    .context("start improvement check")?;
  let _guard = GroupGuard::new(child.id());
  let stdout = child.stdout.take().context("check stdout was not piped")?;
  let stderr = child.stderr.take().context("check stderr was not piped")?;
  let output = timeout(Duration::from_secs(seconds), async {
    tokio::try_join!(
      child.wait(),
      capture(stdout, 32 * 1024),
      capture(stderr, 32 * 1024)
    )
  })
  .await;
  let mut result = CheckResult {
    command: command.into(),
    code: None,
    timed_out: output.is_err(),
    elapsed_ms: started.elapsed().as_millis() as u64,
    stdout: String::new(),
    stderr: String::new(),
  };
  if let Ok(output) = output {
    let (status, stdout, stderr) = output.context("capture improvement check")?;
    result.code = status.code();
    for (target, captured) in [(&mut result.stdout, stdout), (&mut result.stderr, stderr)] {
      *target = String::from_utf8_lossy(&captured.bytes).into_owned();
      if captured.truncated {
        target.push_str("\n[output truncated]");
      }
    }
  }
  Ok(result)
}
