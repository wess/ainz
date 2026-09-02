use std::path::PathBuf;

use ainz::{LocalMemory, MemoryStore, tool::ToolContext};
use serde_json::json;

fn context(workspace: &std::path::Path) -> ToolContext {
  ToolContext {
    workspace: workspace.to_path_buf(),
    session_id: uuid::Uuid::now_v7(),
    max_output_bytes: 8192,
  }
}

#[tokio::test]
async fn local_memories_are_scoped_to_their_workspace() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join("memory");
  let here = LocalMemory::with_root(root.clone(), &PathBuf::from("/work/one"));
  let elsewhere = LocalMemory::with_root(root, &PathBuf::from("/work/two"));

  here
    .remember(
      "the release script stamps the version",
      None,
      "project",
      &[],
    )
    .await
    .unwrap();
  here
    .remember("prefers terse commit messages", None, "global", &[])
    .await
    .unwrap();

  let found = here.recall("release version", 5).await.unwrap();
  assert_eq!(found.len(), 1);
  assert!(found[0].body.contains("stamps the version"));

  // the other workspace sees the global memory and not the project one
  let across = elsewhere.recall("", 5).await.unwrap();
  assert_eq!(across.len(), 1);
  assert_eq!(across[0].scope, "global");
}

#[tokio::test]
async fn remembering_replaces_what_it_supersedes() {
  let temp = tempfile::tempdir().unwrap();
  let store = LocalMemory::with_root(temp.path().join("memory"), &PathBuf::from("/work"));

  store
    .remember("deploys run from the deploy branch", None, "project", &[])
    .await
    .unwrap();
  let first = store.recall("deploys", 5).await.unwrap();
  let old = first[0].id.clone();

  store
    .remember(
      "deploys run from main since the branch was retired",
      Some("deploy change"),
      "project",
      std::slice::from_ref(&old),
    )
    .await
    .unwrap();

  let current = store.recall("deploys", 5).await.unwrap();
  assert_eq!(current.len(), 1);
  assert!(current[0].body.contains("since the branch was retired"));
  assert_eq!(current[0].source.as_deref(), Some("deploy change"));
  assert!(store.forget(&old).await.is_err());
}

#[tokio::test]
async fn the_memory_tool_recalls_what_it_remembered() {
  let temp = tempfile::tempdir().unwrap();
  let workspace = temp.path().join("work");
  let store = MemoryStore::Local(LocalMemory::with_root(
    temp.path().join("memory"),
    &workspace,
  ));
  let tool = store.tool();
  let context = context(&workspace);

  tool
    .execute(
      &context,
      json!({"action": "remember", "content": "the staging database is named orbit"}),
    )
    .await
    .unwrap();

  let output = tool
    .execute(
      &context,
      json!({"action": "recall", "query": "staging database"}),
    )
    .await
    .unwrap();
  assert!(output.contains("orbit"), "{output}");

  let missing = tool
    .execute(
      &context,
      json!({"action": "recall", "query": "unrelated words"}),
    )
    .await
    .unwrap();
  assert_eq!(missing, "no memories matched");
}

#[tokio::test]
async fn memory_off_refuses_to_write() {
  let store = MemoryStore::Off;
  assert!(store.recall("anything", 5).await.unwrap().is_empty());
  assert!(
    store
      .remember("a fact", None, "project", &[])
      .await
      .is_err()
  );
}
