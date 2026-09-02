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

#[tokio::test]
async fn skills_in_the_other_harness_layout_serve_their_bundled_files() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".claude/skills/deploy");
  tokio::fs::create_dir_all(root.join("scripts"))
    .await
    .unwrap();
  tokio::fs::write(
    root.join("SKILL.md"),
    "---\nname: deploy\ndescription: Ship a build\n---\n\nFollow scripts/run.sh.\n",
  )
  .await
  .unwrap();
  tokio::fs::write(root.join("scripts/run.sh"), "#!/bin/sh\necho shipping\n")
    .await
    .unwrap();
  tokio::fs::write(root.join("reference.md"), "the long version")
    .await
    .unwrap();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };

  let catalog = SkillCatalog::discover(temp.path()).await.unwrap();
  let tool = catalog.tool();
  assert!(catalog.skills.iter().any(|skill| skill.name == "deploy"));

  // the skill sits outside the workspace, so the listing is how the model learns what it can read
  let loaded = tool
    .execute(&context, json!({"name": "deploy"}))
    .await
    .unwrap();
  assert!(loaded.contains("Follow scripts/run.sh."));
  assert!(loaded.contains("reference.md"));
  assert!(loaded.contains("scripts/run.sh"));

  let bundled = tool
    .execute(
      &context,
      json!({"name": "deploy", "file": "scripts/run.sh"}),
    )
    .await
    .unwrap();
  assert!(bundled.contains("echo shipping"));

  let escape = tool
    .execute(
      &context,
      json!({"name": "deploy", "file": "../../../etc/hosts"}),
    )
    .await
    .unwrap_err();
  assert!(format!("{escape:#}").contains("not a file bundled with skill deploy"));
}
