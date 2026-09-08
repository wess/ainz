use super::{Capability, capture};
use crate::{process::GroupGuard, workspace};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeSet, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

pub(super) struct Host {
  workspace: PathBuf,
  capabilities: BTreeSet<Capability>,
  timeout: Duration,
  max_output_bytes: usize,
}
impl Host {
  pub(super) fn new(
    workspace: PathBuf,
    capabilities: &[Capability],
    timeout: Duration,
    max_output_bytes: usize,
  ) -> Self {
    Self {
      workspace,
      capabilities: capabilities.iter().copied().collect(),
      timeout,
      max_output_bytes,
    }
  }
  fn require(&self, capability: Capability) -> Result<()> {
    if !self.capabilities.contains(&capability) {
      bail!("{capability:?} capability is required");
    }
    Ok(())
  }

  pub(super) async fn host_read(&mut self, input: &str) -> Result<String> {
    self.require(Capability::WorkspaceRead)?;
    let file = workspace::open(&self.workspace, input, false, false).await?;
    let mut bytes = Vec::new();
    file
      .take(self.max_output_bytes.saturating_add(1) as u64)
      .read_to_end(&mut bytes)
      .await?;
    if bytes.len() > self.max_output_bytes {
      bail!("file exceeds the host transfer limit");
    }
    String::from_utf8(bytes).context("file was not UTF-8")
  }

  pub(super) async fn host_write(&mut self, input: &str, content: &str) -> Result<()> {
    self.require(Capability::WorkspaceWrite)?;
    if content.len() > self.max_output_bytes {
      bail!("content exceeds the host transfer limit");
    }
    workspace::write(&self.workspace, input, content.as_bytes()).await?;
    Ok(())
  }

  // NB: process_exec is full user authority: the shell inherits the host environment
  pub(super) async fn host_run(&mut self, command: &str) -> Result<String> {
    self.require(Capability::ProcessExec)?;
    let mut child = Command::new("sh")
      .args(["-c", command])
      .current_dir(&self.workspace)
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .process_group(0)
      .spawn()?;
    let guard = GroupGuard::new(child.id());
    let stdout = child.stdout.take().context("command stdout unavailable")?;
    let stderr = child.stderr.take().context("command stderr unavailable")?;
    let run = async {
      let (status, stdout, stderr) = tokio::try_join!(
        child.wait(),
        capture(stdout, self.max_output_bytes),
        capture(stderr, self.max_output_bytes)
      )?;
      if stdout.truncated || stderr.truncated {
        bail!("command output exceeded the host transfer limit");
      }
      let mut output = String::from_utf8(stdout.bytes).context("stdout was not UTF-8")?;
      output.push_str(&String::from_utf8(stderr.bytes).context("stderr was not UTF-8")?);
      output.push_str(&format!("\n[exit {}]", status.code().unwrap_or(-1)));
      Result::<String>::Ok(output)
    };
    let output = timeout(self.timeout, run)
      .await
      .context("command timed out")??;
    guard.disarm();
    Ok(output)
  }

  pub(super) async fn host_fetch(&mut self, url: &str) -> Result<String> {
    self.require(Capability::Network)?;
    let url = reqwest::Url::parse(url)?;
    if !matches!(url.scheme(), "http" | "https") {
      bail!("only HTTP and HTTPS URLs are supported");
    }
    // approved plugin network capability includes local services, unlike the web-only tool
    let client = reqwest::Client::builder()
      .connect_timeout(Duration::from_secs(15))
      .build()?;
    let response = client
      .get(url)
      .timeout(self.timeout)
      .send()
      .await?
      .error_for_status()?;
    let bytes = crate::output::response(response, self.max_output_bytes).await?;
    String::from_utf8(bytes).context("response was not UTF-8")
  }
}
