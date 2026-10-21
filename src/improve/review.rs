use std::{collections::BTreeMap, path::Path};

// dependencies
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tokio::{fs, process::Command};

// project
use super::{ImprovementReport, snapshot};
use crate::LocalSkills;

pub async fn review_hash(root: &Path) -> Result<String> {
  let mut hash = Sha256::new();
  for name in ["report.json", "changes.patch"] {
    let bytes = fs::read(root.join(name)).await?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
  }
  Ok(format!("{:x}", hash.finalize()))
}

pub(super) async fn skill_hashes(root: &Path) -> Result<BTreeMap<String, String>> {
  let proposals = LocalSkills::with_root(root.join("skills"));
  let mut hashes = BTreeMap::new();
  for skill in proposals.proposed().await? {
    crate::learn::check_name(&skill.name)?;
    ensure!(
      skill.path == proposals.proposed_dir().join(&skill.name).join("SKILL.md"),
      "skill name does not match its proposal directory"
    );
    hashes.insert(
      skill.name,
      format!("{:x}", Sha256::digest(fs::read(skill.path).await?)),
    );
  }
  Ok(hashes)
}

pub async fn review(root: &Path) -> Result<String> {
  let report: ImprovementReport =
    serde_json::from_slice(&fs::read(root.join("report.json")).await?)?;
  let patch = fs::read_to_string(root.join("changes.patch"))
    .await
    .unwrap_or_default();
  let mut text = format!(
    "improvement {}\nsource: {}\nchecks: {}\n{}\n",
    report.id,
    report.source.display(),
    if report.checks_passed {
      "passed"
    } else {
      "failed or incomplete"
    },
    report.output
  );
  if let Some(error) = report.agent_error {
    text.push_str(&format!("run error: {error}\n"));
  }
  if let Some(behavior) = &report.behavior {
    text.push_str(&format!(
      "\nbehavior: baseline {}/{}, candidate {}/{}\n",
      behavior
        .baseline
        .cases
        .iter()
        .filter(|c| c.passed())
        .count(),
      crate::eval::case_ids().len(),
      behavior
        .candidate
        .cases
        .iter()
        .filter(|c| c.passed())
        .count(),
      crate::eval::case_ids().len()
    ));
    if let Err(error) = behavior.validate(
      &report.source_hash,
      report.candidate_hash.as_deref(),
      &report.skill_hashes,
    ) {
      text.push_str(&format!("behavioral acceptance blocked: {error:#}\n"));
    }
    for (phase, result) in [
      ("baseline", &behavior.baseline),
      ("candidate", &behavior.candidate),
    ] {
      for case in result.cases.iter().filter(|case| !case.passed()) {
        text.push_str(&format!(
          "{phase} {}: {}\n",
          case.id,
          crate::tool::truncate(case.failures.join("; "), 2048)
        ));
      }
    }
  } else {
    text.push_str("\nbehavioral acceptance blocked: no evaluation\n");
  }
  text.push_str(&crate::tool::truncate(patch, 64 * 1024));
  let skills = LocalSkills::with_root(root.join("skills"))
    .proposed()
    .await?;
  if !report.changed_evaluations.is_empty() {
    text.push_str(
      "\nexisting acceptance tests or check scripts changed; automatic acceptance is blocked\n",
    );
  }
  for skill in skills {
    text.push_str(&format!("\nskill {}: {}\n", skill.name, skill.description));
    text.push_str(&crate::tool::truncate(
      fs::read_to_string(skill.path).await?,
      8192,
    ));
  }
  text = crate::tool::truncate(text, 128 * 1024);
  text.push_str(&format!("\nfull evidence: {}", root.display()));
  Ok(text)
}

/// Refuses stale source or modified artifacts before applying a checked proposal.
pub async fn approve(root: &Path, installed: &LocalSkills) -> Result<String> {
  ensure!(
    !root.join("approved.json").exists(),
    "this improvement was already approved"
  );
  let report: ImprovementReport =
    serde_json::from_slice(&fs::read(root.join("report.json")).await?)?;
  ensure!(
    report.checks_passed
      && report.agent_error.is_none()
      && report.baseline_source_unchanged == Some(true)
      && report.check_source_unchanged == Some(true)
      && !report.candidate.is_empty()
      && report.candidate.iter().all(super::CheckResult::passed),
    "only a completed, checked improvement can be approved"
  );
  ensure!(
    report.harness_version == env!("CARGO_PKG_VERSION"),
    "the harness version changed; run a new experiment"
  );
  ensure!(
    report.changed_evaluations.is_empty(),
    "existing acceptance tests or check scripts changed; keep those fixed and run a new experiment"
  );
  report.semantic_status()?;
  report
    .behavior
    .as_ref()
    .context("behavioral evaluation is missing")?
    .verify_context(root)
    .await?;
  crate::eval::EvalContext::load(&root.join("evaluation"))
    .await?
    .verify_skills(&report.workspace)
    .await?;
  ensure!(
    skill_hashes(root).await? == report.skill_hashes,
    "the proposed skills changed since evaluation; run a new experiment"
  );
  let git = snapshot::repository(&report.source).await?;
  let files = snapshot::files(&report.source, &git).await?;
  ensure!(
    snapshot::hash(&report.source, &files).await? == report.source_hash,
    "the source changed since evaluation; run a new experiment"
  );
  let files = snapshot::files(&root.join("review"), &git).await?;
  ensure!(
    Some(snapshot::hash(&root.join("review"), &files).await?) == report.candidate_hash,
    "the checked candidate changed; run a new experiment"
  );
  let patch = fs::read(root.join("changes.patch")).await?;
  ensure!(
    Some(format!("{:x}", Sha256::digest(&patch))) == report.patch_hash
      && snapshot::patch(root).await? == patch,
    "the patch changed since evaluation; run a new experiment"
  );
  let proposals = LocalSkills::with_root(root.join("skills"));
  let skills = proposals.proposed().await?;
  // check all names and conflicts before changing source or installing any skill
  for skill in &skills {
    crate::learn::check_name(&skill.name)?;
    let path = installed.root().join(&skill.name);
    ensure!(
      !path.exists(),
      "skill {} is already installed; review its correction separately",
      skill.name
    );
  }
  if !patch.is_empty() {
    for check in [true, false] {
      let mut command = Command::new("git");
      command.arg("apply").arg("--binary");
      if check {
        command.arg("--check");
      }
      let output = command
        .arg("--")
        .arg(root.join("changes.patch"))
        .current_dir(&report.source)
        .kill_on_drop(true)
        .output()
        .await
        .context("apply improvement")?;
      ensure!(
        output.status.success(),
        "could not apply improvement: {}",
        String::from_utf8_lossy(&output.stderr)
      );
    }
  }
  for skill in &skills {
    installed.approve_from(&skill.name, &proposals).await?;
  }
  fs::write(root.join("approved.json"), serde_json::to_vec_pretty(&serde_json::json!({
    "id": report.id, "source": report.source, "source_hash": report.source_hash,
    "candidate_hash": report.candidate_hash, "skills": skills.iter().map(|s| &s.name).collect::<Vec<_>>()
  }))?).await?;
  Ok(format!(
    "applied harness source changes and approved {} skills; rebuild Ainz to use source changes",
    skills.len()
  ))
}
