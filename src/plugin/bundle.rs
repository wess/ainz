use std::{
  collections::BTreeMap,
  path::{Component, Path},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncReadExt};

const MAX_SOURCE: usize = 4 * 1024 * 1024;

pub(super) struct Bundle {
  #[cfg(feature = "lua")]
  pub entry: String,
  #[cfg(feature = "lua")]
  pub files: BTreeMap<String, Vec<u8>>,
  pub digest: String,
}

pub(super) async fn load(root: &Path, entry: &Path) -> Result<Bundle> {
  if entry.is_absolute()
    || entry
      .components()
      .any(|part| !matches!(part, Component::Normal(_)))
  {
    bail!("Lua runtime.path must be relative to the plugin directory");
  }
  let mut pending = vec![root.to_path_buf()];
  let mut files = BTreeMap::new();
  let mut size = 0;
  while let Some(directory) = pending.pop() {
    let mut entries = fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
      let path = entry.path();
      let kind = entry.file_type().await?;
      if kind.is_dir() {
        pending.push(path);
        continue;
      }
      if path.extension().is_none_or(|ext| ext != "lua") {
        continue;
      }
      if !kind.is_file() {
        bail!("Lua sources must be regular files, not symlinks");
      }
      let name = path
        .strip_prefix(root)?
        .to_str()
        .context("Lua source path must be UTF-8")?
        .to_string();
      let file = crate::workspace::open(root, &name, false, false).await?;
      let mut source = Vec::new();
      file
        .take(MAX_SOURCE.saturating_sub(size) as u64 + 1)
        .read_to_end(&mut source)
        .await?;
      size += source.len();
      if size > MAX_SOURCE {
        bail!("Lua sources exceed the {MAX_SOURCE} byte limit");
      }
      if files.len() >= 1024 {
        bail!("Lua bundle exceeds 1024 files");
      }
      files.insert(name, source);
    }
  }
  let entry = entry
    .to_str()
    .context("Lua entry path must be UTF-8")?
    .to_string();
  if !files.contains_key(&entry) {
    bail!("Lua runtime.path must name a bundled .lua file");
  }
  let mut hash = Sha256::new();
  for (name, source) in &files {
    hash.update(name.as_bytes());
    hash.update([0]);
    hash.update((source.len() as u64).to_le_bytes());
    hash.update(source);
  }
  Ok(Bundle {
    #[cfg(feature = "lua")]
    entry,
    #[cfg(feature = "lua")]
    files,
    digest: format!("{:x}", hash.finalize()),
  })
}
