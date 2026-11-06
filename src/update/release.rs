use std::{
  os::unix::fs::PermissionsExt,
  path::{Path, PathBuf},
  time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::process::Command;

use super::version::valid;

const CHECK: Duration = Duration::from_secs(10);
const DOWNLOAD: Duration = Duration::from_secs(300);
const STARTUP: Duration = Duration::from_secs(10);

/// The published target this build can be replaced by.
pub fn target() -> Option<&'static str> {
  match (std::env::consts::ARCH, std::env::consts::OS) {
    ("aarch64", "macos") => Some("aarch64-apple-darwin"),
    ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
    ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
    _ => None,
  }
}

pub fn archive(version: &str, target: &str) -> String {
  format!("ainz-{version}-{target}.tar.gz")
}

fn client() -> Result<Client> {
  Client::builder()
    .connect_timeout(CHECK)
    .user_agent(concat!("ainz/", env!("CARGO_PKG_VERSION")))
    .build()
    .context("build update client")
}

/// The newest published version. The latest page redirects to its tag, the same answer
/// `install.sh` reads, and unlike the API it has no anonymous rate limit to run into.
pub async fn latest(base: &str) -> Result<String> {
  let response = client()?
    .head(format!("{base}/latest"))
    .timeout(CHECK)
    .send()
    .await
    .context("check for a new release")?
    .error_for_status()
    .context("check for a new release")?;
  let tag = response
    .url()
    .path_segments()
    .and_then(|mut segments| segments.next_back())
    .unwrap_or_default();
  let version = tag.strip_prefix('v').unwrap_or(tag);
  ensure!(valid(version), "unreadable release tag: {tag}");
  Ok(version.to_string())
}

async fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
  let bytes = client
    .get(url)
    .timeout(DOWNLOAD)
    .send()
    .await
    .with_context(|| format!("download {url}"))?
    .error_for_status()
    .with_context(|| format!("download {url}"))?
    .bytes()
    .await
    .with_context(|| format!("download {url}"))?;
  Ok(bytes.to_vec())
}

// removes the staging directory however the install ends
struct Staging(PathBuf);

impl Drop for Staging {
  fn drop(&mut self) {
    drop(std::fs::remove_dir_all(&self.0));
  }
}

/// Replaces `exe` with a published release, or leaves it exactly as it was. Staged beside the
/// target so the final rename stays on one filesystem: a session running the old file keeps it,
/// and a launch at any moment finds one whole binary.
pub async fn install(base: &str, version: &str, exe: &Path) -> Result<()> {
  let target = target().context("no release is published for this platform")?;
  let directory = exe.parent().context("the executable has no directory")?;
  let name = archive(version, target);
  let url = format!("{base}/download/v{version}/{name}");

  let client = client()?;
  let bytes = fetch(&client, &url).await?;
  let sidecar = fetch(&client, &format!("{url}.sha256")).await?;
  let sidecar = String::from_utf8_lossy(&sidecar);
  let expected = sidecar
    .split_whitespace()
    .next()
    .context("the checksum file is empty")?;
  let actual = format!("{:x}", Sha256::digest(&bytes));
  ensure!(
    actual.eq_ignore_ascii_case(expected),
    "checksum mismatch for {name}"
  );

  let staging = Staging(directory.join(format!(".ainzupdate{}", std::process::id())));
  tokio::fs::create_dir_all(&staging.0)
    .await
    .with_context(|| format!("write to {}", directory.display()))?;
  let packed = staging.0.join(&name);
  tokio::fs::write(&packed, bytes)
    .await
    .with_context(|| format!("write {}", packed.display()))?;
  let unpacked = Command::new("tar")
    .arg("-xzf")
    .arg(&packed)
    .arg("-C")
    .arg(&staging.0)
    .arg("ainz")
    .output()
    .await
    .context("run tar")?;
  if !unpacked.status.success() {
    bail!(
      "unpack {name}: {}",
      String::from_utf8_lossy(&unpacked.stderr).trim()
    );
  }

  let staged = staging.0.join("ainz");
  tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
    .await
    .context("mark the new binary executable")?;
  confirm(&staged, version).await?;
  tokio::fs::rename(&staged, exe)
    .await
    .with_context(|| format!("replace {}", exe.display()))
}

// a binary that cannot start here must never take the place of one that can
async fn confirm(binary: &Path, version: &str) -> Result<()> {
  let run = Command::new(binary)
    .arg("--version")
    .kill_on_drop(true)
    .output();
  let output = tokio::time::timeout(STARTUP, run)
    .await
    .context("the new binary did not start in time")?
    .context("the new binary did not start")?;
  let reported = String::from_utf8_lossy(&output.stdout);
  ensure!(
    output.status.success() && reported.trim() == format!("ainz {version}"),
    "the new binary reported {:?}, expected ainz {version}",
    reported.trim()
  );
  Ok(())
}
