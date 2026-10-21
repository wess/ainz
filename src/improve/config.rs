use std::path::PathBuf;

// dependencies
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ImproveConfig {
  pub enabled: bool,
  pub source: Option<PathBuf>,
  pub checks: Vec<String>,
  pub every_runs: usize,
  pub cooldown_seconds: u64,
  pub max_steps: usize,
  pub max_seconds: u64,
  pub budget_usd: f64,
}

impl Default for ImproveConfig {
  fn default() -> Self {
    Self {
      enabled: false,
      source: None,
      checks: vec![
        "cargo test --no-default-features".into(),
        "cargo fmt --all -- --check".into(),
      ],
      every_runs: 5,
      cooldown_seconds: 86400,
      max_steps: 12,
      max_seconds: 900,
      budget_usd: 0.5,
    }
  }
}

impl ImproveConfig {
  pub fn validate(&self) -> Result<()> {
    ensure!(self.every_runs > 0, "improve.every_runs must be positive");
    ensure!(
      !self.checks.is_empty() && self.checks.iter().all(|c| !c.trim().is_empty()),
      "improve.checks needs at least one nonempty command"
    );
    ensure!(
      self.max_steps > 0 && (1..=7200).contains(&self.max_seconds),
      "improve limits need positive steps and 1..=7200 seconds"
    );
    ensure!(
      self.budget_usd.is_finite() && self.budget_usd > 0.0,
      "improve.budget_usd must be positive and finite"
    );
    Ok(())
  }
}
