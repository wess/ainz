use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::fs;

fn relative(input: &str) -> Result<PathBuf> {
  let path = PathBuf::from(input);
  if path.is_absolute() {
    bail!("absolute paths are outside the workspace");
  }
  let mut clean = PathBuf::new();
  for component in path.components() {
    match component {
      std::path::Component::Normal(value) => clean.push(value),
      std::path::Component::CurDir => {}
      _ => bail!("path escapes the workspace"),
    }
  }
  Ok(clean)
}

pub async fn existing(workspace: &Path, input: &str) -> Result<PathBuf> {
  let workspace = fs::canonicalize(workspace).await?;
  let path = fs::canonicalize(workspace.join(relative(input)?)).await?;
  ensure_contained(&workspace, &path)?;
  Ok(path)
}

pub async fn writable(workspace: &Path, input: &str) -> Result<PathBuf> {
  let workspace = fs::canonicalize(workspace).await?;
  let candidate = workspace.join(relative(input)?);
  if fs::symlink_metadata(&candidate).await.is_ok() {
    let path = fs::canonicalize(&candidate).await?;
    ensure_contained(&workspace, &path)?;
    return Ok(path);
  }
  let mut ancestor = candidate.parent().context("path has no parent")?;
  while !fs::try_exists(ancestor).await? {
    ancestor = ancestor.parent().context("path has no existing ancestor")?;
  }
  let resolved = fs::canonicalize(ancestor).await?;
  ensure_contained(&workspace, &resolved)?;
  Ok(resolved.join(candidate.strip_prefix(ancestor)?))
}

fn ensure_contained(workspace: &Path, path: &Path) -> Result<()> {
  if !path.starts_with(workspace) {
    bail!("path escapes the workspace");
  }
  Ok(())
}

// resolve and open relative to a directory handle; a path check followed by an ambient
// open would allow a symlink swap to redirect the operation after the check.
pub async fn open(workspace: &Path, input: &str, write: bool, create: bool) -> Result<fs::File> {
  let root = workspace.to_path_buf();
  let path = relative(input)?;
  let file = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
    use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
    let dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    if create
      && let Some(parent) = path.parent()
      && !parent.as_os_str().is_empty()
    {
      dir
        .create_dir_all(parent)
        .context("path escapes the workspace or its parent cannot be created")?;
    }
    let mut options = OpenOptions::new();
    options
      .read(true)
      .write(write)
      .create(create)
      .custom_flags(libc::O_NONBLOCK);
    let file = dir
      .open_with(&path, &options)
      .context("path escapes the workspace or cannot be opened")?;
    if !file.metadata()?.is_file() {
      bail!("workspace file must be a regular file");
    }
    Ok(file.into_std())
  })
  .await??;
  Ok(fs::File::from_std(file))
}

pub async fn write(workspace: &Path, input: &str, content: &[u8]) -> Result<()> {
  use tokio::io::AsyncWriteExt;
  let mut file = open(workspace, input, true, true).await?;
  file.set_len(0).await?;
  file.write_all(content).await?;
  file.flush().await?;
  Ok(())
}
