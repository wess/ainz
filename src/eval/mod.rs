use std::{collections::BTreeMap, path::Path};

// dependencies
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// project
use crate::protocol::{ToolCall, Usage};

mod cases;
mod context;
mod grade;
mod process;
mod run;

pub use context::{EvalContext, LoadedSkill, proposed};
pub use run::evaluate;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaseResult {
  pub id: String,
  pub failures: Vec<String>,
  pub usage: Usage,
}

impl CaseResult {
  pub fn passed(&self) -> bool {
    self.failures.is_empty()
  }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SuiteResult {
  pub suite_hash: String,
  pub context_hash: String,
  pub source_hash: String,
  pub cases: Vec<CaseResult>,
  pub error: Option<String>,
  pub usage: Usage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Comparison {
  pub baseline: SuiteResult,
  pub candidate: SuiteResult,
  pub skill_hashes: BTreeMap<String, String>,
}

impl Comparison {
  pub fn validate(
    &self,
    source_hash: &str,
    candidate_hash: Option<&str>,
    skills: &BTreeMap<String, String>,
  ) -> Result<()> {
    ensure!(
      self.baseline.suite_hash == suite_hash() && self.candidate.suite_hash == suite_hash(),
      "behavioral evaluation suite changed; run a new experiment"
    );
    ensure!(
      self.baseline.context_hash == self.candidate.context_hash
        && !self.baseline.context_hash.is_empty(),
      "behavioral evaluations did not use the same frozen context"
    );
    for result in [&self.baseline, &self.candidate] {
      ensure!(
        result.error.is_none(),
        "behavioral evaluation was unavailable: {}",
        result.error.as_deref().unwrap_or_default()
      );
      ensure!(
        result.cases.len() == cases::CASES.len()
          && result
            .cases
            .iter()
            .zip(cases::CASES)
            .all(|(actual, expected)| actual.id == expected.id),
        "behavioral evaluation is missing, reordered, or duplicates a required case"
      );
    }
    ensure!(
      self.baseline.source_hash == source_hash
        && Some(self.candidate.source_hash.as_str()) == candidate_hash,
      "behavioral results belong to different source snapshots"
    );
    ensure!(
      &self.skill_hashes == skills,
      "proposed skills differ from the behavioral evaluation"
    );
    let failed: Vec<_> = self
      .candidate
      .cases
      .iter()
      .filter(|c| !c.passed())
      .map(|c| c.id.as_str())
      .collect();
    ensure!(
      failed.is_empty(),
      "behavioral regression or unmet contract: {}",
      failed.join(", ")
    );
    Ok(())
  }

  pub async fn verify_context(&self, root: &Path) -> Result<()> {
    let context = root.join("evaluation");
    ensure!(
      EvalContext::hash(&context).await? == self.baseline.context_hash,
      "frozen behavioral context changed since evaluation"
    );
    EvalContext::load(&context)
      .await?
      .verify_instructions(&context)
      .await?;
    Ok(())
  }
}

pub fn suite_hash() -> String {
  let mut hash = Sha256::new();
  for text in [
    include_str!("cases.rs"),
    include_str!("grade.rs"),
    include_str!("driver.rs"),
    include_str!("run.rs"),
    include_str!("context.rs"),
    include_str!("process.rs"),
    include_str!("mod.rs"),
  ] {
    hash.update((text.len() as u64).to_le_bytes());
    hash.update(text.as_bytes());
  }
  format!("{:x}", hash.finalize())
}

pub fn case_ids() -> Vec<&'static str> {
  cases::CASES.iter().map(|c| c.id).collect()
}

pub fn add_usage(total: &mut Usage, next: &Usage) {
  total.input_tokens = total.input_tokens.saturating_add(next.input_tokens);
  total.output_tokens = total.output_tokens.saturating_add(next.output_tokens);
  total.cost_usd = match (total.cost_usd, next.cost_usd) {
    (Some(left), Some(right)) => Some(left + right),
    (left, right) => left.or(right),
  };
}

#[derive(Deserialize)]
struct DriverResult {
  completed: bool,
  error: Option<String>,
  usage: Usage,
  calls: Vec<ToolCall>,
}
