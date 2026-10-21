use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub(crate) fn improvements() -> Option<(notify::RecommendedWatcher, mpsc::Receiver<()>)> {
  let store = ainz::improve::automatic::AutomaticStore::default_store().ok()?;
  store.update(|_| Ok(())).ok()?;
  crate::live::attention::watch(&store.root).ok()
}

pub(crate) struct Branch {
  pub updates: mpsc::Receiver<Option<String>>,
  task: tokio::task::JoinHandle<()>,
}
impl Drop for Branch {
  fn drop(&mut self) {
    self.task.abort();
  }
}

async fn gitdir(workspace: &Path) -> Option<PathBuf> {
  for directory in workspace.ancestors() {
    let path = directory.join(".git");
    if tokio::fs::metadata(&path)
      .await
      .ok()
      .is_some_and(|m| m.is_dir())
    {
      return Some(path);
    }
    if let Ok(text) = tokio::fs::read_to_string(&path).await
      && let Some(path) = text.trim().strip_prefix("gitdir: ")
    {
      return Some(directory.join(path));
    }
  }
  None
}

async fn read(path: &Path) -> Option<String> {
  let head = tokio::fs::read_to_string(path.join("HEAD")).await.ok()?;
  let head = head.trim();
  let label = match head.strip_prefix("ref: refs/heads/") {
    Some(branch) => branch.to_string(),
    None if head.len() >= 8 && head.chars().all(|c| c.is_ascii_hexdigit()) => {
      format!("detached {}", &head[..8])
    }
    _ => return None,
  };
  Some(
    label
      .chars()
      .filter(|c| !c.is_control())
      .take(100)
      .collect(),
  )
}

pub(crate) async fn branch(workspace: &Path) -> Option<Branch> {
  let path = gitdir(workspace).await?;
  let (watcher, mut changes) = crate::live::attention::watch(&path).ok()?;
  let (send, updates) = mpsc::channel(1);
  let task = tokio::spawn(async move {
    let _watcher = watcher;
    let mut last = read(&path).await;
    if send.send(last.clone()).await.is_err() {
      return;
    }
    while changes.recv().await.is_some() {
      while changes.try_recv().is_ok() {}
      let next = read(&path).await;
      if next != last {
        last = next.clone();
        if send.send(next).await.is_err() {
          break;
        }
      }
    }
  });
  Some(Branch { updates, task })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn a_worktree_git_file_resolves_its_own_head_from_nested_directories() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("tree");
    let metadata = root.path().join("metadata");
    tokio::fs::create_dir_all(tree.join("src")).await.unwrap();
    tokio::fs::create_dir_all(&metadata).await.unwrap();
    tokio::fs::write(tree.join(".git"), "gitdir: ../metadata\n")
      .await
      .unwrap();
    tokio::fs::write(metadata.join("HEAD"), "ref: refs/heads/review/session\n")
      .await
      .unwrap();
    let dir = gitdir(&tree.join("src")).await.unwrap();
    assert_eq!(read(&dir).await.as_deref(), Some("review/session"));
    tokio::fs::write(metadata.join("HEAD"), "a123456789abcdef\n")
      .await
      .unwrap();
    assert_eq!(read(&dir).await.as_deref(), Some("detached a1234567"));
  }
}
