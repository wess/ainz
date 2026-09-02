use agentx::{SkillCatalog, tool::ToolContext};
use serde_json::json;

#[tokio::test]
async fn skills_are_discovered_but_loaded_on_demand() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".agentx/skills/review");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(
    root.join("SKILL.md"),
    "---\nname: review\ndescription: Inspect a change\n---\n\n# Review\n\nRead the diff.\n",
  )
  .await
  .unwrap();

  let catalog = SkillCatalog::discover(temp.path()).await.unwrap();
  let skill = catalog
    .skills
    .iter()
    .find(|skill| skill.name == "review")
    .unwrap();
  assert_eq!(skill.description, "Inspect a change");
  let tool = catalog.tool();
  assert!(tool.spec().description.contains("review: Inspect a change"));

  let loaded = tool
    .execute(
      &ToolContext {
        workspace: temp.path().into(),
        session_id: uuid::Uuid::nil(),
        max_output_bytes: 1024,
      },
      json!({"name": "review"}),
    )
    .await
    .unwrap();
  assert!(loaded.contains("Read the diff."));
}
