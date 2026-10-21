use ainz::{LocalSkills, Teacher, improve::approve};
use tokio::fs;

#[tokio::test]
async fn changed_source_patch_candidate_and_skills_are_rejected_before_installation() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  super::repository(&source).await;
  let mut experiment = super::prepare(&source, &temp.path().join("experiments")).await;
  experiment
    .baseline(&["grep -q fixed src/lib.rs".into()], 2)
    .await
    .unwrap();
  fs::write(experiment.candidate().join("src/lib.rs"), "fixed\n")
    .await
    .unwrap();
  let proposals = LocalSkills::with_root(experiment.root.join("skills"));
  Teacher::Local(proposals.clone())
    .teach(
      "verify",
      "Verify a repair",
      "reproduce, fix, check",
      "project",
      None,
    )
    .await
    .unwrap();
  experiment.finish(Ok("done".into()), 2).await.unwrap();
  super::certify(&mut experiment).await;
  let installed = LocalSkills::with_root(temp.path().join("installed"));

  let context = experiment.root.join("evaluation/context.json");
  let frozen = fs::read(&context).await.unwrap();
  fs::write(&context, "altered context").await.unwrap();
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("frozen behavioral context changed")
  );
  fs::write(&context, frozen).await.unwrap();

  let added = source.join(".ainz/skills/added");
  fs::create_dir_all(&added).await.unwrap();
  fs::write(
    added.join("SKILL.md"),
    "---\nname: added\ndescription: new instructions\n---\nchanged context\n",
  )
  .await
  .unwrap();
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("installed skill context changed")
  );
  fs::remove_dir_all(source.join(".ainz")).await.unwrap();

  fs::write(source.join("src/other.rs"), "unrelated change")
    .await
    .unwrap();
  let error = approve(&experiment.root, &installed).await.unwrap_err();
  assert!(error.to_string().contains("source changed"), "{error}");
  fs::remove_file(source.join("src/other.rs")).await.unwrap();
  let patch = fs::read(experiment.root.join("changes.patch"))
    .await
    .unwrap();
  fs::write(experiment.root.join("changes.patch"), "altered")
    .await
    .unwrap();
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("patch changed")
  );
  fs::write(experiment.root.join("changes.patch"), &patch)
    .await
    .unwrap();
  fs::write(experiment.root.join("review/src/lib.rs"), "altered")
    .await
    .unwrap();
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("candidate changed")
  );
  fs::write(experiment.root.join("review/src/lib.rs"), "fixed\n")
    .await
    .unwrap();
  let skill = proposals.proposed_dir().join("verify/SKILL.md");
  let text = fs::read(&skill).await.unwrap();
  fs::write(
    &skill,
    "---\nname: verify\ndescription: changed\n---\nchanged\n",
  )
  .await
  .unwrap();
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("skills changed")
  );
  assert!(!installed.root().exists());
  fs::write(&skill, text).await.unwrap();

  approve(&experiment.root, &installed).await.unwrap();
  assert_eq!(
    fs::read_to_string(source.join("src/lib.rs")).await.unwrap(),
    "fixed\n"
  );
  assert!(installed.root().join("verify/SKILL.md").exists());
  assert!(experiment.root.join("approved.json").exists());
  assert!(approve(&experiment.root, &installed).await.is_err());
}

#[tokio::test]
async fn changing_existing_acceptance_tests_blocks_acceptance_despite_passing_checks() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  super::repository(&source).await;
  fs::create_dir(source.join("tests")).await.unwrap();
  fs::write(source.join("tests/accept.rs"), "original expectation")
    .await
    .unwrap();
  fs::create_dir_all(source.join("src/eval")).await.unwrap();
  fs::write(source.join("src/eval/cases.rs"), "fixed task expectations")
    .await
    .unwrap();
  let mut experiment = super::prepare(&source, &temp.path().join("experiments")).await;
  experiment.baseline(&["true".into()], 2).await.unwrap();
  fs::write(
    experiment.candidate().join("tests/accept.rs"),
    "weakened expectation",
  )
  .await
  .unwrap();
  fs::write(
    experiment.candidate().join("src/eval/cases.rs"),
    "weakened oracle",
  )
  .await
  .unwrap();
  experiment.finish(Ok("done".into()), 2).await.unwrap();
  assert!(experiment.report.checks_passed);
  assert_eq!(experiment.report.changed_evaluations.len(), 2);
  let installed = LocalSkills::with_root(temp.path().join("installed"));
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("acceptance tests")
  );
  assert_eq!(
    fs::read_to_string(source.join("tests/accept.rs"))
      .await
      .unwrap(),
    "original expectation"
  );
}

#[tokio::test]
async fn successful_source_checks_without_behavioral_evidence_cannot_be_approved() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  super::repository(&source).await;
  let mut experiment = super::prepare(&source, &temp.path().join("experiments")).await;
  experiment.baseline(&["true".into()], 2).await.unwrap();
  experiment.finish(Ok("done".into()), 2).await.unwrap();
  let installed = LocalSkills::with_root(temp.path().join("installed"));
  assert!(
    approve(&experiment.root, &installed)
      .await
      .unwrap_err()
      .to_string()
      .contains("behavioral evaluation is required")
  );
}
