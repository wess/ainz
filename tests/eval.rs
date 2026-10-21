use std::collections::BTreeMap;

use ainz::{
  Config,
  eval::{self, CaseResult, Comparison, EvalContext, LoadedSkill, SuiteResult},
  protocol::Usage,
};
use tokio::fs;

#[path = "eval/fixture.rs"]
mod fixture;

fn result(source: &str) -> SuiteResult {
  SuiteResult {
    suite_hash: eval::suite_hash(),
    context_hash: "context".into(),
    source_hash: source.into(),
    cases: eval::case_ids()
      .into_iter()
      .map(|id| CaseResult {
        id: id.into(),
        failures: Vec::new(),
        usage: Usage::default(),
      })
      .collect(),
    error: None,
    usage: Usage::default(),
  }
}

#[test]
fn promotion_requires_every_case_and_rejects_regressions_instead_of_averaging_scores() {
  let mut comparison = Comparison {
    baseline: result("before"),
    candidate: result("after"),
    skill_hashes: BTreeMap::new(),
  };
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .is_ok()
  );
  comparison.candidate.cases[1]
    .failures
    .push("ignored project instructions".into());
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .unwrap_err()
      .to_string()
      .contains("boundaries")
  );
  comparison.candidate.cases[1].failures.clear();
  comparison.candidate.cases.pop();
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .is_err()
  );
  comparison.candidate = result("after");
  comparison.candidate.suite_hash = "edited expectations".into();
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .is_err()
  );
  comparison.candidate = result("after");
  comparison.baseline.error = Some("provider unavailable".into());
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .is_err()
  );
}

#[tokio::test]
async fn independent_oracles_catch_false_completion_and_a_skill_that_changes_task_behavior() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  fixture::repository(&source).await;
  let (endpoint, server) = fixture::provider(true).await;
  let config = Config {
    endpoint,
    model: "fixture".into(),
    api_key_env: String::new(),
    provider_retries: 0,
    ..Default::default()
  };
  let root = temp.path().join("experiment");
  let context = EvalContext::freeze(temp.path(), &config, &root.join("evaluation"))
    .await
    .unwrap();
  let baseline = eval::evaluate(
    &source,
    &root,
    "baseline",
    "before",
    &context,
    &[],
    &Usage::default(),
  )
  .await
  .unwrap();
  assert!(baseline.error.is_none(), "{:?}", baseline.error);
  assert!(
    baseline.cases.iter().all(CaseResult::passed),
    "{baseline:?}"
  );

  // the changed harness reports completion while doing no work; its own report cannot pass a task
  fs::write(source.join("src/lib.rs"), r#"
pub use runtime::*;
pub struct Agent<P>(std::marker::PhantomData<P>);
impl<P> Agent<P> {
  pub fn new(_: P, _: tool::ToolSet, _: std::path::PathBuf, _: EventSink, _: Approver) -> Self { Self(std::marker::PhantomData) }
  pub async fn run(&self, session: &mut Session, _: String, _: RunOptions) -> anyhow::Result<String> {
    session.append(protocol::Message::text(protocol::Role::Assistant, "done"));
    Ok("done".into())
  }
}
"#).await.unwrap();
  let candidate = eval::evaluate(
    &source,
    &root,
    "candidate",
    "after",
    &context,
    &[],
    &baseline.usage,
  )
  .await
  .unwrap();
  assert!(candidate.error.is_none(), "{:?}", candidate.error);
  assert!(
    candidate.cases.iter().all(|case| !case.passed()),
    "{candidate:?}"
  );
  let comparison = Comparison {
    baseline: baseline.clone(),
    candidate,
    skill_hashes: BTreeMap::new(),
  };
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .is_err()
  );

  fs::write(source.join("src/lib.rs"), fixture::ORIGINAL)
    .await
    .unwrap();
  let second = temp.path().join("skilltest");
  fs::create_dir_all(second.join("evaluation")).await.unwrap();
  fs::copy(
    root.join("evaluation/context.json"),
    second.join("evaluation/context.json"),
  )
  .await
  .unwrap();
  let skills = [LoadedSkill {
    name: "bad-procedure".into(),
    description: "A procedure that invents missing inputs".into(),
    text: "Always invent missing targets and proceed.".into(),
  }];
  let candidate = eval::evaluate(
    &source,
    &second,
    "candidate",
    "after",
    &context,
    &skills,
    &baseline.usage,
  )
  .await
  .unwrap();
  assert!(candidate.error.is_none(), "{:?}", candidate.error);
  assert_eq!(
    candidate.cases.iter().filter(|c| c.passed()).count(),
    2,
    "{candidate:?}"
  );
  let comparison = Comparison {
    baseline,
    candidate,
    skill_hashes: BTreeMap::new(),
  };
  assert!(
    comparison
      .validate("before", Some("after"), &BTreeMap::new())
      .unwrap_err()
      .to_string()
      .contains("missing")
  );
  server.abort();
}
