use std::path::Path;

use ainz::{
  Session, SessionStore,
  improve::{Evidence, Improvement, evidence},
  protocol::{Message, Role},
};
use tokio::{fs, process::Command};

async fn certify(experiment: &mut Improvement) {
  // artifact tests substitute scoring evidence; real evaluation is covered in tests/eval.rs
  use ainz::eval::{CaseResult, Comparison, EvalContext, SuiteResult};
  fs::create_dir_all(experiment.root.join("evaluation"))
    .await
    .unwrap();
  EvalContext::freeze(
    &experiment.report.workspace,
    &ainz::Config::default(),
    &experiment.root.join("evaluation"),
  )
  .await
  .unwrap();
  let result = SuiteResult {
    suite_hash: ainz::eval::suite_hash(),
    context_hash: EvalContext::hash(&experiment.root.join("evaluation"))
      .await
      .unwrap(),
    source_hash: experiment.report.source_hash.clone(),
    error: None,
    usage: Default::default(),
    cases: ainz::eval::case_ids()
      .into_iter()
      .map(|id| CaseResult {
        id: id.into(),
        failures: Vec::new(),
        usage: Default::default(),
      })
      .collect(),
  };
  let mut candidate = result.clone();
  candidate.source_hash = experiment.report.candidate_hash.clone().unwrap();
  experiment.report.behavior = Some(Comparison {
    baseline: result,
    candidate,
    skill_hashes: experiment.report.skill_hashes.clone(),
  });
  experiment.save().await.unwrap();
}

#[cfg(feature = "cli")]
#[path = "improve/cli.rs"]
mod cli;

#[path = "improve/automatic.rs"]
mod automatic;
#[cfg(feature = "cli")]
#[path = "improve/background.rs"]
mod background;

#[path = "improve/drift.rs"]
mod drift;
#[cfg(feature = "cli")]
#[path = "eval/fixture.rs"]
mod evaluation;

async fn repository(path: &Path) {
  fs::create_dir_all(path.join("src")).await.unwrap();
  fs::write(
    path.join("Cargo.toml"),
    "[package]\nname = \"ainz\"\nversion = \"0.0.0\"\n",
  )
  .await
  .unwrap();
  fs::write(path.join("src/lib.rs"), "broken\n")
    .await
    .unwrap();
  fs::write(path.join(".gitignore"), "target/\n.env\n")
    .await
    .unwrap();
  for args in [vec!["init", "-q"], vec!["add", "."]] {
    assert!(
      Command::new("git")
        .args(args)
        .current_dir(path)
        .status()
        .await
        .unwrap()
        .success()
    );
  }
}

async fn prepare(source: &Path, out: &Path) -> Improvement {
  Improvement::prepare(
    source,
    source,
    out,
    Evidence {
      sessions: Vec::new(),
      text: String::new(),
    },
    Some("fix the observed broken value".into()),
  )
  .await
  .unwrap()
}

#[tokio::test]
async fn an_experiment_preserves_working_changes_and_produces_an_applicable_patch() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  // the snapshot is the current working tree, not the index or a prior commit
  fs::write(source.join("src/lib.rs"), "current\n")
    .await
    .unwrap();
  fs::write(source.join(".env"), "private").await.unwrap();
  fs::create_dir(source.join("target")).await.unwrap();
  fs::write(source.join("target/generated"), "ignored")
    .await
    .unwrap();
  let mut experiment = prepare(&source, &temp.path().join("experiments")).await;
  assert_eq!(
    fs::read_to_string(experiment.candidate().join("src/lib.rs"))
      .await
      .unwrap(),
    "current\n"
  );
  assert!(!experiment.candidate().join(".env").exists());
  assert!(!experiment.candidate().join("target").exists());
  experiment
    .baseline(&["test \"$(cat src/lib.rs)\" = fixed".into()], 2)
    .await
    .unwrap();
  assert!(!experiment.report.baseline[0].passed());
  fs::write(experiment.candidate().join("src/lib.rs"), "fixed\n")
    .await
    .unwrap();
  fs::write(experiment.candidate().join("src/new.rs"), "new\n")
    .await
    .unwrap();
  experiment
    .finish(Ok("fixed the cause".into()), 2)
    .await
    .unwrap();
  assert!(experiment.report.checks_passed);
  assert!(experiment.report.changed);
  assert_eq!(experiment.report.check_source_unchanged, Some(true));
  assert_eq!(
    fs::read_to_string(source.join("src/lib.rs")).await.unwrap(),
    "current\n"
  );
  assert!(!source.join("src/new.rs").exists());
  let patch = fs::read_to_string(experiment.root.join("changes.patch"))
    .await
    .unwrap();
  assert!(patch.contains("src/new.rs"), "{patch}");
  let output = Command::new("git")
    .args(["apply", "--check"])
    .arg(experiment.root.join("changes.patch"))
    .current_dir(&source)
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}\n{patch}",
    String::from_utf8_lossy(&output.stderr)
  );
  let report: serde_json::Value =
    serde_json::from_slice(&fs::read(experiment.root.join("report.json")).await.unwrap()).unwrap();
  assert_eq!(report["checks_passed"], true);
  assert_eq!(report["baseline"][0]["code"], 1);
  assert_eq!(report["candidate"][0]["code"], 0);
}

#[tokio::test]
async fn check_failures_timeouts_and_agent_failures_never_validate_a_candidate() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  for (command, agent_failed) in [("exit 7", false), ("sleep 5", false), ("true", true)] {
    let mut experiment = prepare(&source, &temp.path().join("experiments")).await;
    experiment.baseline(&[command.into()], 1).await.unwrap();
    let result = if agent_failed {
      Err(anyhow::anyhow!("provider failed"))
    } else {
      Ok("done".into())
    };
    experiment.finish(result, 1).await.unwrap();
    assert!(!experiment.report.checks_passed);
    if command == "sleep 5" {
      assert!(experiment.report.candidate[0].timed_out);
    }
    if agent_failed {
      assert_eq!(
        experiment.report.agent_error.as_deref(),
        Some("provider failed")
      );
    }
  }
}

#[tokio::test]
async fn checks_that_rewrite_source_do_not_verify_the_original_proposal() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  let mut experiment = prepare(&source, &temp.path().join("experiments")).await;
  experiment
    .baseline(&["echo rewritten > src/lib.rs".into()], 2)
    .await
    .unwrap();
  experiment.finish(Ok("done".into()), 2).await.unwrap();
  assert!(experiment.report.candidate[0].passed());
  assert_eq!(experiment.report.check_source_unchanged, Some(false));
  assert!(!experiment.report.checks_passed);
  assert_eq!(
    fs::read_to_string(experiment.root.join("baseline/src/lib.rs"))
      .await
      .unwrap(),
    "broken\n"
  );
}

#[tokio::test]
async fn source_symlinks_and_output_inside_the_checkout_are_rejected() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  let evidence = Evidence {
    sessions: Vec::new(),
    text: String::new(),
  };
  assert!(
    Improvement::prepare(
      &source,
      &source,
      &source.join("experiments"),
      evidence.clone(),
      Some("fix".into())
    )
    .await
    .is_err()
  );
  std::os::unix::fs::symlink(temp.path(), source.join("escape")).unwrap();
  assert!(
    Improvement::prepare(
      &source,
      &source,
      &temp.path().join("out"),
      evidence,
      Some("fix".into())
    )
    .await
    .is_err()
  );
}

#[tokio::test]
async fn evidence_uses_only_active_branches_from_the_requested_workspace() {
  let temp = tempfile::tempdir().unwrap();
  let store = SessionStore::new(temp.path().join("sessions"));
  let mut session = Session::new(temp.path().join("workspace"));
  let user = session.append(Message::text(Role::User, "the observed failure"));
  session.append(Message::text(Role::Assistant, "abandoned branch"));
  session.checkout(Some(user)).unwrap();
  session.append(Message::text(Role::Assistant, "active branch"));
  store.save(&session).await.unwrap();
  let mut other = Session::new(temp.path().join("other"));
  other.append(Message::text(Role::User, "unrelated private content"));
  store.save(&other).await.unwrap();
  let evidence = evidence(&store, &session.workspace, 5).await.unwrap();
  assert_eq!(evidence.sessions, vec![session.id]);
  assert!(evidence.text.contains("active branch"));
  assert!(!evidence.text.contains("abandoned branch"));
  assert!(!evidence.text.contains("unrelated private content"));
}

#[tokio::test]
async fn a_candidate_check_cannot_rely_on_ignored_files_missing_from_its_patch() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  let mut experiment = prepare(&source, &temp.path().join("experiments")).await;
  experiment
    .baseline(&["test -f target/hidden".into()], 2)
    .await
    .unwrap();
  fs::create_dir(experiment.candidate().join("target"))
    .await
    .unwrap();
  fs::write(experiment.candidate().join("target/hidden"), "present")
    .await
    .unwrap();
  experiment.finish(Ok("done".into()), 2).await.unwrap();
  assert!(!experiment.report.checks_passed);
}

#[tokio::test]
async fn a_mutated_baseline_cannot_lend_a_clean_comparison_to_the_candidate() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  let mut experiment = prepare(&source, &temp.path().join("experiments")).await;
  experiment
    .baseline(&["echo fixed > src/lib.rs".into()], 2)
    .await
    .unwrap();
  fs::write(experiment.candidate().join("src/lib.rs"), "fixed\n")
    .await
    .unwrap();
  experiment.finish(Ok("done".into()), 2).await.unwrap();
  assert_eq!(experiment.report.baseline_source_unchanged, Some(false));
  assert_eq!(experiment.report.check_source_unchanged, Some(true));
  assert!(!experiment.report.checks_passed);
}

#[tokio::test]
async fn noisy_checks_drain_both_pipes_and_bound_the_retained_output() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  repository(&source).await;
  let mut experiment = prepare(&source, &temp.path().join("experiments")).await;
  let command = "awk 'BEGIN {for (i=0; i<10000; i++) {print \"stdout flood\"; print \"stderr flood\" > \"/dev/stderr\"}}'";
  experiment.baseline(&[command.into()], 5).await.unwrap();
  experiment.finish(Ok("done".into()), 5).await.unwrap();
  assert!(experiment.report.checks_passed);
  for check in experiment
    .report
    .baseline
    .iter()
    .chain(&experiment.report.candidate)
  {
    assert!(check.stdout.len() < 33 * 1024);
    assert!(check.stderr.len() < 33 * 1024);
    assert!(check.stdout.ends_with("[output truncated]"));
    assert!(check.stderr.ends_with("[output truncated]"));
  }
}

#[tokio::test]
async fn evidence_keeps_the_failed_command_and_its_exit_status_after_long_output() {
  let temp = tempfile::tempdir().unwrap();
  let store = SessionStore::new(temp.path().join("sessions"));
  let mut session = Session::new(temp.path().join("workspace"));
  let mut call = Message::text(Role::Assistant, "checking");
  call.tool_calls.push(ainz::protocol::ToolCall {
    id: "check".into(),
    name: "shell".into(),
    arguments: serde_json::json!({"command": "cargo test regressions"}),
  });
  session.append(call);
  session.append(Message::tool(
    "check",
    format!("{}\nfailed: regression\n[exit 1]", "output ".repeat(1000)),
  ));
  store.save(&session).await.unwrap();
  let evidence = evidence(&store, &session.workspace, 1).await.unwrap();
  assert!(evidence.text.contains("cargo test regressions"));
  assert!(evidence.text.contains("failed: regression\n[exit 1]"));
  assert!(evidence.text.contains("Tool check:"));
  assert!(evidence.text.len() < 2000);
}
