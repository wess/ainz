use ainz::PromptCatalog;

#[tokio::test]
async fn prompt_templates_expand_arguments_by_scope() {
  let temp = tempfile::tempdir().unwrap();
  let parent = temp.path().join("project");
  let workspace = parent.join("child");
  tokio::fs::create_dir_all(parent.join(".ainz/prompts"))
    .await
    .unwrap();
  tokio::fs::create_dir_all(workspace.join(".ainz/prompts"))
    .await
    .unwrap();
  tokio::fs::write(parent.join(".ainz/prompts/review.md"), "parent {{args}}")
    .await
    .unwrap();
  tokio::fs::write(
    workspace.join(".ainz/prompts/review.md"),
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

#[tokio::test]
async fn commands_in_the_other_harness_layout_expand_dollar_placeholders() {
  let temp = tempfile::tempdir().unwrap();
  let commands = temp.path().join(".claude/commands");
  tokio::fs::create_dir_all(commands.join("api"))
    .await
    .unwrap();
  tokio::fs::write(
    commands.join("review.md"),
    "---\ndescription: Review a path\nargument-hint: <path> [focus]\n---\nInspect $1 with $ARGUMENTS.",
  )
  .await
  .unwrap();
  tokio::fs::write(commands.join("api/audit.md"), "Audit the API surface.")
    .await
    .unwrap();

  let catalog = PromptCatalog::discover(temp.path()).await.unwrap();
  let review = catalog
    .prompts
    .iter()
    .find(|prompt| prompt.name == "review")
    .unwrap();

  assert_eq!(review.description, "Review a path");
  assert_eq!(review.hint.as_deref(), Some("<path> [focus]"));
  // a subdirectory namespaces its commands the way the other harness does
  assert!(
    catalog
      .prompts
      .iter()
      .any(|prompt| prompt.name == "api:audit")
  );

  let output = catalog
    .expand("review", &["src".into(), "carefully".into()])
    .await
    .unwrap();
  assert_eq!(output, "Inspect src with src carefully.");
}
