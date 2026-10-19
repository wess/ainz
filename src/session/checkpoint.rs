use super::{Session, SessionStore};
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;
impl SessionStore {
  /// A named snapshot of a session, kept beside the sessions so a rewind has something to
  /// come back to. Checkpoints are separate files: taking one never touches the live
  /// transcript, and restoring one never destroys the checkpoint.
  pub async fn checkpoint(&self, session: &Session, label: &str) -> Result<PathBuf> {
    let label = checkpoint_label(label)?;
    let directory = self.checkpoint_dir(session.id);
    fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{label}.json"));
    let temp = path.with_extension(format!("{}.tmp", Uuid::now_v7()));
    let mut file = fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(&temp)
      .await?;
    let result = async {
      file.write_all(&serde_json::to_vec_pretty(session)?).await?;
      file.sync_all().await?;
      drop(file);
      fs::rename(&temp, &path).await?;
      anyhow::Ok(())
    }
    .await;
    if result.is_err() {
      let _ = fs::remove_file(&temp).await;
    }
    result?;
    Ok(path)
  }

  /// The session as it stood at a checkpoint. The caller decides what to do with it: saving
  /// it back is the rewind, and dropping it leaves the live session alone.
  pub async fn restore(&self, id: Uuid, label: &str) -> Result<Session> {
    let label = checkpoint_label(label)?;
    let path = self.checkpoint_dir(id).join(format!("{label}.json"));
    let data = match fs::read(&path).await {
      Ok(data) => data,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
        bail!("session {id} has no checkpoint named {label}")
      }
      Err(error) => return Err(error).with_context(|| format!("read checkpoint {label}")),
    };
    let session: Session =
      serde_json::from_slice(&data).with_context(|| format!("parse checkpoint {label}"))?;
    if session.id != id {
      bail!("checkpoint {label} holds session {}, not {id}", session.id);
    }
    Ok(session)
  }

  /// The labels a session has been checkpointed under, in name order.
  pub async fn checkpoints(&self, id: Uuid) -> Result<Vec<String>> {
    let mut entries = match fs::read_dir(self.checkpoint_dir(id)).await {
      Ok(entries) => entries,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
      Err(error) => return Err(error).context("read checkpoint directory"),
    };
    let mut labels = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
      let path = entry.path();
      if path
        .extension()
        .is_some_and(|extension| extension == "json")
        && let Some(label) = path.file_stem().and_then(|stem| stem.to_str())
      {
        labels.push(label.to_string());
      }
    }
    labels.sort();
    Ok(labels)
  }

  fn checkpoint_dir(&self, id: Uuid) -> PathBuf {
    self.root.join("checkpoints").join(id.to_string())
  }
}
fn checkpoint_label(label: &str) -> Result<String> {
  let trimmed = label.trim();
  if trimmed.is_empty() {
    bail!("a checkpoint needs a label");
  }
  if trimmed.len() > 64 {
    bail!("a checkpoint label is at most 64 characters");
  }
  if !trimmed
    .chars()
    .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
  {
    bail!("a checkpoint label may only hold letters, digits, dashes and underscores: {trimmed}");
  }
  Ok(trimmed.to_string())
}
