use std::{
  collections::BTreeSet,
  os::unix::fs::PermissionsExt,
  path::{Component, Path, PathBuf},
  process::Stdio,
  time::Duration,
};

// dependencies
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tokio::{fs, process::Command};

// project
use crate::{output::capture, process::GroupGuard};

pub(super) async fn repository(source: &Path) -> Result<PathBuf> {
  let output = Command::new("git")
    .kill_on_drop(true)
    .args(["rev-parse", "--show-toplevel", "--absolute-git-dir"])
    .current_dir(source)
    .output()
    .await
    .context("locate harness repository")?;
  ensure!(
    output.status.success(),
    "could not inspect the harness checkout with Git ({}): {}",
    output.status,
    String::from_utf8_lossy(&output.stderr).trim()
  );
  let text = String::from_utf8(output.stdout)?;
  let mut lines = text.lines();
  let root = PathBuf::from(lines.next().context("repository root is missing")?);
  ensure!(
    root.canonicalize()? == source,
    "source must be the repository root"
  );
  Ok(PathBuf::from(
    lines.next().context("repository directory is missing")?,
  ))
}

pub(super) async fn files(root: &Path, git: &Path) -> Result<Vec<PathBuf>> {
  let output = Command::new("git")
    .kill_on_drop(true)
    .arg("--git-dir")
    .arg(git)
    .arg("--work-tree")
    .arg(root)
    .args([
      "ls-files",
      "--cached",
      "--others",
      "--exclude-standard",
      "-z",
    ])
    .current_dir(root)
    .output()
    .await
    .context("list harness source")?;
  ensure!(output.status.success(), "could not list harness source");
  let mut files = BTreeSet::new();
  for name in output
    .stdout
    .split(|byte| *byte == 0)
    .filter(|name| !name.is_empty())
  {
    let path = PathBuf::from(std::str::from_utf8(name).context("source path is not UTF-8")?);
    ensure!(
      path
        .components()
        .all(|part| matches!(part, Component::Normal(_))),
      "source path must stay inside the checkout"
    );
    if path.components().any(|part| {
      let name = part.as_os_str().to_string_lossy();
      name == ".env" || name.starts_with(".env.") || name == ".git"
    }) {
      continue;
    }
    match fs::symlink_metadata(root.join(&path)).await {
      Ok(meta) => {
        ensure!(
          !meta.is_symlink(),
          "source symlinks are not supported: {}",
          path.display()
        );
        // also reject a symlink in a parent directory
        let real = fs::canonicalize(root.join(&path)).await?;
        ensure!(
          real.starts_with(root),
          "source path leaves checkout: {}",
          path.display()
        );
        if meta.is_file() {
          files.insert(path);
        }
      }
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
      Err(error) => return Err(error).context("inspect harness source"),
    }
  }
  Ok(files.into_iter().collect())
}

pub(super) async fn copy(root: &Path, destination: &Path, files: &[PathBuf]) -> Result<String> {
  fs::create_dir_all(destination).await?;
  for path in files {
    let bytes = fs::read(root.join(path)).await?;
    let target = destination.join(path);
    fs::create_dir_all(target.parent().context("source file has no parent")?).await?;
    fs::write(&target, &bytes).await?;
    fs::set_permissions(&target, fs::metadata(root.join(path)).await?.permissions()).await?;
  }
  hash(destination, files).await
}

pub(super) async fn hash(root: &Path, files: &[PathBuf]) -> Result<String> {
  let mut hash = Sha256::new();
  for path in files {
    let bytes = fs::read(root.join(path)).await?;
    hash.update(path.to_string_lossy().as_bytes());
    hash.update([0]);
    hash.update(
      fs::metadata(root.join(path))
        .await?
        .permissions()
        .mode()
        .to_le_bytes(),
    );
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(&bytes);
  }
  Ok(format!("{:x}", hash.finalize()))
}

pub(super) async fn patch(root: &Path) -> Result<Vec<u8>> {
  let mut child = Command::new("git")
    .args([
      "-c",
      "core.filemode=true",
      "diff",
      "--no-index",
      "--binary",
      "--no-ext-diff",
      "--no-prefix",
      "--",
      "baseline",
      "review",
    ])
    .current_dir(root)
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true)
    .process_group(0)
    .spawn()
    .context("start candidate diff")?;
  let _guard = GroupGuard::new(child.id());
  let stdout = child.stdout.take().context("diff stdout was not piped")?;
  let (status, output) = tokio::time::timeout(Duration::from_secs(30), async {
    tokio::try_join!(child.wait(), capture(stdout, 8 * 1024 * 1024))
  })
  .await
  .context("candidate diff timed out")??;
  ensure!(
    matches!(status.code(), Some(0 | 1)),
    "candidate diff failed"
  );
  ensure!(!output.truncated, "candidate patch exceeds 8 MiB");
  Ok(output.bytes)
}
