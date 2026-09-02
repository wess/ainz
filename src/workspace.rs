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
  if fs::try_exists(&candidate).await? {
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
