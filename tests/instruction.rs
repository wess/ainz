use agentx::instruction;

#[tokio::test]
async fn both_instruction_filenames_are_read_with_the_nearest_last() {
  let temp = tempfile::tempdir().unwrap();
  let workspace = temp.path().join("project");
  tokio::fs::create_dir_all(&workspace).await.unwrap();
  tokio::fs::write(temp.path().join("AGENTS.md"), "outer agents rule")
    .await
    .unwrap();
  tokio::fs::write(workspace.join("AGENTS.md"), "inner agents rule")
    .await
    .unwrap();
  tokio::fs::write(workspace.join("CLAUDE.md"), "inner second rule")
    .await
    .unwrap();

  let text = instruction::load(&workspace).await.unwrap();
  let outer = text.find("outer agents rule").unwrap();
  let inner = text.find("inner agents rule").unwrap();
  let second = text.find("inner second rule").unwrap();

  assert!(
    outer < inner,
    "a parent directory must come before the workspace"
  );
  assert!(inner < second, "both filenames are read at the same level");
}
