use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The last answer from the release page, kept so most launches never touch the network.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct State {
  pub checked_at: u64,
  pub latest: Option<String>,
}

/// A missing or damaged file only costs one extra check.
pub async fn load(path: &Path) -> State {
  match tokio::fs::read(path).await {
    Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
    Err(_) => State::default(),
  }
}

pub async fn save(path: &Path, state: &State) -> Result<()> {
  if let Some(parent) = path.parent() {
    tokio::fs::create_dir_all(parent)
      .await
      .with_context(|| format!("create {}", parent.display()))?;
  }
  tokio::fs::write(path, serde_json::to_vec(state)?)
    .await
    .with_context(|| format!("write {}", path.display()))
}
