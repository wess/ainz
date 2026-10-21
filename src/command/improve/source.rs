use std::{
  path::{Path, PathBuf},
  process::Stdio,
  time::Duration,
};

// dependencies
use anyhow::{Context, Result, ensure};
use tokio::{fs, process::Command};

pub(super) async fn resolve(workspace: &Path, configured: Option<&Path>) -> Result<PathBuf> {
  if let Some(path) = configured {
    return Ok(path.canonicalize()?);
  }
  if is_source(workspace).await {
    return Ok(workspace.canonicalize()?);
  }
  let built_from = Path::new(env!("CARGO_MANIFEST_DIR"));
  if is_source(built_from).await && built_from.join(".git").exists() {
    return Ok(built_from.canonicalize()?);
  }
  let parent = dirs::data_local_dir()
    .context("could not locate the data directory")?
    .join("ainz/source");
  fs::create_dir_all(&parent).await?;
  let root = parent.join(env!("CARGO_PKG_VERSION"));
  if is_source(&root).await {
    return Ok(root.canonicalize()?);
  }
  let temp = parent.join(uuid::Uuid::now_v7().to_string());
  let mut child = Command::new("git")
    .args(["clone", "--quiet", "--depth", "1", "--branch"])
    .arg(format!("v{}", env!("CARGO_PKG_VERSION")))
    .arg(env!("CARGO_PKG_REPOSITORY"))
    .arg(&temp)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .kill_on_drop(true)
    .spawn()
    .context("download matching harness source")?;
  let status = tokio::time::timeout(Duration::from_secs(120), child.wait())
    .await
    .context("source download timed out")??;
  ensure!(
    status.success(),
    "could not download source for this release; set improve.source to a local checkout"
  );
  ensure!(
    is_source(&temp).await,
    "downloaded source does not match this release"
  );
  let manifest: toml::Value = toml::from_str(&fs::read_to_string(temp.join("Cargo.toml")).await?)?;
  ensure!(
    manifest
      .get("package")
      .and_then(|p| p.get("version"))
      .and_then(|v| v.as_str())
      == Some(env!("CARGO_PKG_VERSION")),
    "downloaded source version does not match the executable"
  );
  fs::rename(temp, &root).await?;
  Ok(root.canonicalize()?)
}

async fn is_source(root: &Path) -> bool {
  let Ok(text) = fs::read_to_string(root.join("Cargo.toml")).await else {
    return false;
  };
  let Ok(manifest) = toml::from_str::<toml::Value>(&text) else {
    return false;
  };
  manifest
    .get("package")
    .and_then(|p| p.get("name"))
    .and_then(|n| n.as_str())
    == Some("ainz")
}
