use ainz::{LocalSkills, SkillCatalog, Teacher, tool::ToolContext};
use serde_json::json;

#[tokio::test]
async fn a_taught_skill_waits_for_approval_before_it_is_discovered() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join("skills");
  let skills = LocalSkills::with_root(root.clone());
  let teacher = Teacher::Local(skills.clone());
  let context = ToolContext {
    workspace: temp.path().to_path_buf(),
    session_id: uuid::Uuid::now_v7(),
    max_output_bytes: 8192,
  };

  let message = teacher
    .tool()
    .execute(
      &context,
      json!({
        "action": "teach", "name": "cut-a-release",
        "description": "Bump, stamp, tag, and watch the release build",
        "instructions": "1. bump Cargo.toml\n2. run scripts/version.sh\n3. tag it"
      }),
    )
    .await
    .unwrap();
  assert!(message.contains("ainz skills approve"), "{message}");

  // a proposal is listed but not discovered, so it costs nothing in a session
  let proposed = skills.proposed().await.unwrap();
  assert_eq!(proposed.len(), 1);
  assert_eq!(proposed[0].name, "cut-a-release");
  let discovered = SkillCatalog::discover_with_roots(temp.path(), std::slice::from_ref(&root))
    .await
    .unwrap();
  assert!(
    discovered
      .skills
      .iter()
      .all(|skill| skill.name != "cut-a-release")
  );

  skills.approve("cut-a-release").await.unwrap();
  let discovered = SkillCatalog::discover_with_roots(temp.path(), &[root])
    .await
    .unwrap();
  let skill = discovered
    .skills
    .iter()
    .find(|skill| skill.name == "cut-a-release")
    .expect("approved skill is discovered");
  assert!(skill.description.starts_with("Bump, stamp"));
}

#[tokio::test]
async fn a_correction_replaces_the_installed_copy_and_keeps_the_old_one() {
  let temp = tempfile::tempdir().unwrap();
  let skills = LocalSkills::with_root(temp.path().join("skills"));
  let teacher = Teacher::Local(skills.clone());

  teacher
    .teach(
      "debug-a-flaky-test",
      "Find why a test only fails in CI",
      "run it with --nocapture",
      "project",
      None,
    )
    .await
    .unwrap();
  let installed = skills.approve("debug-a-flaky-test").await.unwrap();

  let message = teacher
    .revise(
      "debug-a-flaky-test",
      "run it under the same TZ as CI, then with --nocapture",
      None,
      Some("the timezone was the actual cause"),
    )
    .await
    .unwrap();
  assert!(message.contains("replaced version"), "{message}");

  let text = tokio::fs::read_to_string(installed.join("SKILL.md"))
    .await
    .unwrap();
  assert!(text.contains("same TZ as CI"));
  // the description survives a correction that does not supply a new one
  assert!(text.contains("Find why a test only fails in CI"));
}

#[tokio::test]
async fn proposals_can_be_rejected_and_names_are_checked() {
  let temp = tempfile::tempdir().unwrap();
  let skills = LocalSkills::with_root(temp.path().join("skills"));
  let teacher = Teacher::Local(skills.clone());

  assert!(
    teacher
      .teach("Not A Name", "d", "i", "project", None)
      .await
      .is_err()
  );
  teacher
    .teach("write-a-migration", "d", "i", "project", None)
    .await
    .unwrap();
  skills.reject("write-a-migration").await.unwrap();
  assert!(skills.proposed().await.unwrap().is_empty());
  assert!(skills.approve("write-a-migration").await.is_err());
}
