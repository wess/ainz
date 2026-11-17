use std::{process::Stdio, time::Duration};

// dependencies
use anyhow::{Context, Result};
use tokio::process::Command;

// project
use crate::{output::capture, process::GroupGuard};

pub(super) struct Output {
  pub success: bool,
  pub stdout: Vec<u8>,
  pub stderr: String,
}

pub(super) async fn run(command: Command, seconds: u64) -> Result<Output> {
  // a build directory the case was pointed at lies outside its workspace, and has to be written
  let writable: Vec<std::path::PathBuf> = command
    .as_std()
    .get_envs()
    .filter(|(key, _)| *key == "CARGO_TARGET_DIR")
    .filter_map(|(_, value)| value.map(std::path::PathBuf::from))
    .collect();
  let mut child = crate::sandbox::confine(command, &writable)?
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .process_group(0)
    .spawn()
    .context("start semantic evaluation process")?;
  let _guard = GroupGuard::new(child.id());
  let stdout = child.stdout.take().context("stdout was not piped")?;
  let stderr = child.stderr.take().context("stderr was not piped")?;
  let (status, stdout, stderr) = tokio::time::timeout(Duration::from_secs(seconds), async {
    tokio::try_join!(
      child.wait(),
      capture(stdout, 256 * 1024),
      capture(stderr, 32 * 1024)
    )
  })
  .await
  .context("semantic evaluation timed out")??;
  anyhow::ensure!(
    !stdout.truncated,
    "semantic evaluation output exceeded its limit"
  );
  Ok(Output {
    success: status.success(),
    stdout: stdout.bytes,
    stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
  })
}
