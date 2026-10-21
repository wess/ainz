use std::{path::Path, time::Instant};

// dependencies
use anyhow::{Result, ensure};
use serde_json::json;
use tokio::{fs, process::Command};

// project
use super::{
  CaseResult, DriverResult, EvalContext, LoadedSkill, SuiteResult, add_usage,
  cases::{CASES, Case},
  grade, process, suite_hash,
};
use crate::protocol::Usage;

pub async fn evaluate(
  source: &Path,
  root: &Path,
  phase: &str,
  source_hash: &str,
  context: &EvalContext,
  loaded_skills: &[LoadedSkill],
  spent: &Usage,
) -> Result<SuiteResult> {
  ensure!(
    matches!(phase, "baseline" | "candidate"),
    "unknown evaluation phase"
  );
  let directory = root.join("evaluation").join(phase);
  fs::create_dir_all(&directory).await?;
  let mut result = SuiteResult {
    suite_hash: suite_hash(),
    context_hash: EvalContext::hash(&root.join("evaluation")).await?,
    source_hash: source_hash.into(),
    cases: Vec::new(),
    error: None,
    usage: Usage::default(),
  };
  let execution = async {
    let driver = directory.join("driver");
    fs::create_dir_all(driver.join("src")).await?;
    let manifest = json!({"package":{"name":"behaviorprobe", "version":"0.0.0", "edition":"2024"},
    "workspace":{}, "dependencies":{
      "ainz":{"path":source, "default-features":false}, "anyhow":"1", "serde_json":"1",
      "tokio":{"version":"1", "features":["macros", "rt-multi-thread", "fs", "time"]}
    }});
    fs::write(driver.join("Cargo.toml"), toml::to_string(&manifest)?).await?;
    fs::write(driver.join("src/main.rs"), include_str!("driver.rs")).await?;
    let target = root.join("build").join(phase);
    let mut build = Command::new("cargo");
    build
      .args(["build", "--quiet", "--manifest-path"])
      .arg(driver.join("Cargo.toml"))
      .env("CARGO_TARGET_DIR", &target)
      .current_dir(&driver);
    let compiled = process::run(build, 300).await?;
    ensure!(
      compiled.success,
      "behavioral harness could not build: {}",
      compiled.stderr
    );
    for case in CASES {
      let budget = context.config.budget_usd.unwrap_or(0.5)
        - spent.cost_usd.unwrap_or(0.0)
        - result.usage.cost_usd.unwrap_or(0.0);
      ensure!(
        budget > 0.0,
        "experiment budget exhausted before behavioral evaluation finished"
      );
      let started = Instant::now();
      let (graded, raw) = run_case(
        case,
        &directory.join(case.id),
        &target.join("debug/behaviorprobe"),
        context,
        loaded_skills,
        budget,
      )
      .await?;
      fs::write(directory.join(format!("{}.json", case.id)), &raw).await?;
      fs::write(
        directory.join(format!("{}.time", case.id)),
        started.elapsed().as_millis().to_string(),
      )
      .await?;
      add_usage(&mut result.usage, &graded.usage);
      result.cases.push(graded);
    }
    Ok::<_, anyhow::Error>(())
  }
  .await;
  if let Err(error) = execution {
    result.error = Some(format!("{error:#}"));
  }
  fs::write(
    directory.join("results.json"),
    serde_json::to_vec_pretty(&result)?,
  )
  .await?;
  Ok(result)
}

async fn run_case(
  case: &Case,
  directory: &Path,
  driver: &Path,
  context: &EvalContext,
  loaded_skills: &[LoadedSkill],
  budget: f64,
) -> Result<(CaseResult, Vec<u8>)> {
  ensure!(
    !directory.exists(),
    "behavioral task directory already exists; run a fresh experiment"
  );
  context.verify_instructions(directory).await?;
  let workspace = directory.join("workspace");
  for (path, text) in case.files {
    let path = workspace.join(path);
    fs::create_dir_all(
      path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("fixture has no parent"))?,
    )
    .await?;
    fs::write(path, text).await?;
  }
  for (index, skill) in context.skills.iter().enumerate() {
    let path = workspace.join(".ainz/skills").join(index.to_string());
    fs::create_dir_all(&path).await?;
    fs::write(path.join("SKILL.md"), &skill.text).await?;
  }
  let mut config = context.config.clone();
  config.budget_usd = Some(budget);
  let request = directory.join("request.json");
  fs::write(
    &request,
    serde_json::to_vec(&json!({"config":config, "workspace":workspace,
    "prompt":case.prompt, "skills": context.skills, "loaded_skills":loaded_skills}))?,
  )
  .await?;
  let mut command = Command::new(driver);
  command.arg(&request).current_dir(&workspace);
  let raw = process::run(command, 120).await?;
  ensure!(
    raw.success,
    "behavioral task process failed: {}",
    raw.stderr
  );
  let actual: DriverResult = serde_json::from_slice(&raw.stdout)?;
  let failures = grade::grade(case, &workspace, &actual, &directory.join("oracle")).await?;
  context.verify_instructions(directory).await?;
  Ok((
    CaseResult {
      id: case.id.into(),
      failures,
      usage: actual.usage,
    },
    raw.stdout,
  ))
}
