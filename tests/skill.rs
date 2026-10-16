use ainz::{SkillCatalog, tool::ToolContext};
use serde_json::json;

#[tokio::test]
async fn skills_are_discovered_but_loaded_on_demand() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/skills/review");
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
      &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
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
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 4096);

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

#[tokio::test]
async fn one_unreadable_skill_is_reported_without_losing_the_others() {
  let temp = tempfile::tempdir().unwrap();
  let skills = temp.path().join(".ainz/skills");
  let elsewhere = temp.path().join("shared/linked");
  for directory in [skills.join("good"), skills.join("bad"), elsewhere.clone()] {
    tokio::fs::create_dir_all(&directory).await.unwrap();
  }
  tokio::fs::write(skills.join("good/SKILL.md"), "# Good\n")
    .await
    .unwrap();
  tokio::fs::write(skills.join("bad/SKILL.md"), [0xff, 0xfe, 0x00])
    .await
    .unwrap();
  tokio::fs::write(elsewhere.join("SKILL.md"), "# Linked\n")
    .await
    .unwrap();
  std::os::unix::fs::symlink(&elsewhere, skills.join("linked")).unwrap();
  // stray files where a skill directory would be, the way Finder leaves one
  tokio::fs::write(skills.join(".DS_Store"), "junk")
    .await
    .unwrap();
  tokio::fs::write(skills.join("notes.txt"), "junk")
    .await
    .unwrap();

  let catalog = SkillCatalog::discover(temp.path()).await.unwrap();
  // the user's own skills are discovered too, so only this workspace's are compared
  let mut names: Vec<_> = catalog
    .skills
    .iter()
    .filter(|skill| skill.path.starts_with(temp.path()))
    .map(|skill| skill.name.as_str())
    .collect();
  names.sort_unstable();
  assert_eq!(names, ["good", "linked"]);
  let root = temp.path().to_string_lossy();
  let issues: Vec<_> = catalog
    .issues
    .iter()
    .filter(|issue| issue.contains(root.as_ref()))
    .collect();
  assert_eq!(issues.len(), 1, "{issues:?}");
  assert!(issues[0].contains("bad"), "{issues:?}");
}
