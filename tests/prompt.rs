use agentx::PromptCatalog;

#[tokio::test]
async fn prompt_templates_expand_arguments_by_scope() {
  let temp = tempfile::tempdir().unwrap();
  let parent = temp.path().join("project");
  let workspace = parent.join("child");
  tokio::fs::create_dir_all(parent.join(".agentx/prompts"))
    .await
    .unwrap();
  tokio::fs::create_dir_all(workspace.join(".agentx/prompts"))
    .await
    .unwrap();
  tokio::fs::write(parent.join(".agentx/prompts/review.md"), "parent {{args}}")
    .await
    .unwrap();
  tokio::fs::write(
    workspace.join(".agentx/prompts/review.md"),
    "---\nname: review\ndescription: Review a path\n---\nInspect {{1}} with {{args}}.",
  )
  .await
  .unwrap();

  let catalog = PromptCatalog::discover(&workspace).await.unwrap();
  let prompt = catalog
    .prompts
    .iter()
    .find(|prompt| prompt.name == "review")
    .unwrap();
  assert_eq!(prompt.description, "Review a path");
  let output = catalog
    .expand("review", &["src".into(), "carefully".into()])
    .await
    .unwrap();
  assert_eq!(output, "Inspect src with src carefully.");
}
