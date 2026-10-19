use anyhow::Result;
use std::path::Path;
use tokio::io::AsyncWriteExt;

pub(super) async fn dir(path: &Path) -> Result<()> {
  use std::os::unix::fs::{MetadataExt, PermissionsExt};
  if let Ok(metadata) = tokio::fs::symlink_metadata(path).await {
    anyhow::ensure!(
      metadata.is_dir() && !metadata.is_symlink() && metadata.uid() == unsafe { libc::geteuid() },
      "live directory must belong to this user and must not be a symlink"
    );
  }
  tokio::fs::DirBuilder::new()
    .recursive(true)
    .mode(0o700)
    .create(path)
    .await?;
  tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
  Ok(())
}
pub(super) async fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
  let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
  let result = async {
    let mut file = tokio::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(&temp)
      .await?;
    file.write_all(bytes.as_ref()).await?;
    file.sync_all().await?;
    tokio::fs::rename(&temp, path).await?;
    Ok::<_, anyhow::Error>(())
  }
  .await;
  if result.is_err() {
    let _ = tokio::fs::remove_file(temp).await;
  }
  result
}
