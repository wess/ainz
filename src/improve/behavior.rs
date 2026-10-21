// dependencies
use anyhow::{Context, Result, ensure};

// project
use super::{Improvement, ImprovementReport};
use crate::{
  Config,
  eval::{self, Comparison, EvalContext, SuiteResult},
};

impl ImprovementReport {
  pub fn semantic_status(&self) -> Result<()> {
    self
      .behavior
      .as_ref()
      .context("behavioral evaluation is required before acceptance")?
      .validate(
        &self.source_hash,
        self.candidate_hash.as_deref(),
        &self.skill_hashes,
      )
  }

  pub fn acceptable(&self) -> bool {
    self.checks_passed && self.changed_evaluations.is_empty() && self.semantic_status().is_ok()
  }
}

impl Improvement {
  pub async fn baseline_behavior(&mut self, config: &Config) -> Result<EvalContext> {
    let context = EvalContext::freeze(
      &self.report.workspace,
      config,
      &self.root.join("evaluation"),
    )
    .await?;
    let baseline = eval::evaluate(
      &self.root.join("baseline"),
      &self.root,
      "baseline",
      &self.report.source_hash,
      &context,
      &[],
      &self.report.usage,
    )
    .await?;
    eval::add_usage(&mut self.report.usage, &baseline.usage);
    self.report.behavior = Some(Comparison {
      candidate: SuiteResult {
        suite_hash: baseline.suite_hash.clone(),
        context_hash: baseline.context_hash.clone(),
        source_hash: String::new(),
        cases: Vec::new(),
        error: Some("candidate has not been evaluated".into()),
        usage: Default::default(),
      },
      baseline,
      skill_hashes: Default::default(),
    });
    self.save().await?;
    Ok(context)
  }

  pub async fn candidate_behavior(&mut self, context: &EvalContext) -> Result<()> {
    ensure!(
      self.report.checks_passed && self.report.changed_evaluations.is_empty(),
      "candidate source checks must pass before behavioral evaluation"
    );
    let skills = eval::proposed(&self.root).await?;
    let candidate = eval::evaluate(
      &self.root.join("review"),
      &self.root,
      "candidate",
      self
        .report
        .candidate_hash
        .as_deref()
        .context("candidate hash is missing")?,
      context,
      &skills,
      &self.report.usage,
    )
    .await?;
    eval::add_usage(&mut self.report.usage, &candidate.usage);
    let behavior = self
      .report
      .behavior
      .as_mut()
      .context("baseline behavior is missing")?;
    behavior.candidate = candidate;
    behavior.skill_hashes = self.report.skill_hashes.clone();
    self.save().await
  }
}
