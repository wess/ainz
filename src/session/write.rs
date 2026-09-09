use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use super::{Session, SessionStore};

struct PendingFile(Option<PathBuf>);

impl Drop for PendingFile {
  fn drop(&mut self) {
    if let Some(path) = self.0.take() {
      drop(std::fs::remove_file(path));
    }
  }
}

impl SessionStore {
  /// Atomically replaces a session. Concurrent saves are whole-file, last-writer-wins.
  pub async fn save(&self, session: &Session) -> Result<()> {
    fs::create_dir_all(&self.root).await?;
    let path = self.path(session.id);
    let temp = path.with_extension(format!("{}.tmp", Uuid::now_v7()));
    let data = serde_json::to_vec_pretty(session)?;
    let mut file = fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(&temp)
      .await
      .context("create session checkpoint")?;
    let mut pending = PendingFile(Some(temp.clone()));
    file
      .write_all(&data)
      .await
      .context("write session checkpoint")?;
    file.flush().await?;
    file.sync_all().await.context("sync session checkpoint")?;
    drop(file);
    fs::rename(&temp, &path)
      .await
      .context("replace session checkpoint")?;
    pending.0 = None;
    fs::File::open(&self.root)
      .await?
      .sync_all()
      .await
      .context("sync session directory")?;
    Ok(())
  }
}
