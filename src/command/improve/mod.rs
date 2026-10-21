use std::{
  io::IsTerminal,
  path::{Path, PathBuf},
  sync::Arc,
};

// dependencies
use ainz::{
  Approver, Config, LocalSkills, PermissionMode, deny_all,
  improve::{self, automatic::AutomaticStore},
};
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use uuid::Uuid;

mod background;
mod perform;
mod source;

pub(crate) use background::after_save;

#[derive(Args)]
pub struct ImproveArgs {
  #[command(subcommand)]
  action: Option<ImproveAction>,
  /// Harness checkout; discovered or downloaded automatically when omitted
  #[arg(long, global = true)]
  source: Option<PathBuf>,
  /// Verification commands; defaults to the configured harness checks
  #[arg(long = "check", global = true)]
  checks: Vec<String>,
  #[arg(long)]
  focus: Option<String>,
  #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=20))]
  sessions: u8,
  #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u16).range(1..=3600))]
  timeout: u16,
  #[arg(long)]
  out: Option<PathBuf>,
  #[arg(long, global = true)]
  json: bool,
}

#[derive(Subcommand)]
enum ImproveAction {
  /// Enable automatic experiments after normal saved runs
  #[command(alias = "enable")]
  On,
  /// Stop scheduling new experiments
  #[command(alias = "disable")]
  Off,
  Status,
  /// Review the latest experiment, or one by id
  Review {
    id: Option<Uuid>,
  },
  /// Accept the reviewed patch and its proposed skills together
  Approve {
    id: Option<Uuid>,
  },
  #[command(hide = true)]
  Worker {
    request: PathBuf,
  },
}

impl ImproveArgs {
  fn automatic() -> Self {
    Self {
      action: None,
      source: None,
      checks: Vec::new(),
      focus: None,
      sessions: 5,
      timeout: 300,
      out: None,
      json: true,
    }
  }
}

pub async fn run(workspace: &Path, config: &mut Config, args: ImproveArgs) -> Result<()> {
  match &args.action {
    Some(ImproveAction::On | ImproveAction::Off) => {
      config.improve.enabled = matches!(args.action, Some(ImproveAction::On));
      if let Some(source) = &args.source {
        config.improve.source = Some(source.canonicalize()?);
      }
      if !args.checks.is_empty() {
        config.improve.checks = args.checks.clone();
      }
      config.improve.validate()?;
      config.save().await?;
      println!("{}", status(workspace, config)?);
      return Ok(());
    }
    Some(ImproveAction::Status) => {
      println!("{}", status(workspace, config)?);
      return Ok(());
    }
    Some(ImproveAction::Review { id } | ImproveAction::Approve { id }) => {
      let root = if matches!(args.action, Some(ImproveAction::Approve { .. })) {
        reviewed(workspace, *id).await?
      } else {
        experiment(workspace, *id)?
      };
      let text = if matches!(args.action, Some(ImproveAction::Approve { .. })) {
        accept(&root, config).await?
      } else {
        let text = improve::review(&root).await?;
        mark_reviewed(workspace, &root).await?;
        text
      };
      println!("{text}");
      return Ok(());
    }
    Some(ImproveAction::Worker { request }) => return background::worker(config, request).await,
    None => {}
  }
  config.validate()?;
  ensure!(
    config.permissions != PermissionMode::ReadOnly,
    "improve needs candidate edits; choose ask or auto permissions"
  );
  let approver: Approver = if args.json || !std::io::stdin().is_terminal() {
    deny_all()
  } else {
    Arc::new(crate::app::approve)
  };
  let Some(experiment) = perform::perform(
    workspace,
    config,
    &args,
    crate::app::output_events(args.json),
    approver,
  )
  .await?
  else {
    println!("no saved runs yet; use Ainz normally first");
    return Ok(());
  };
  background::remember_result(workspace, &experiment)?;
  if args.json {
    println!(
      "{}",
      serde_json::json!({"type":"improvement", "path":experiment.root, "report":experiment.report})
    );
  } else {
    println!(
      "\nproposal: {}\nchecks: {}\nreview: ainz improve review\naccept: ainz improve approve",
      experiment.root.display(),
      if experiment.report.acceptable() {
        "passed"
      } else {
        "failed or inconclusive"
      }
    );
  }
  ensure!(
    experiment.report.acceptable(),
    "candidate verification or behavioral evaluation failed; retained the experiment for review"
  );
  Ok(())
}

pub(crate) fn status(workspace: &Path, config: &Config) -> Result<String> {
  let state = AutomaticStore::default_store()?.read()?;
  let project = state.projects.get(workspace);
  Ok(format!(
    "automatic improvement: {}\nnew runs: {} / {}\nlatest: {}",
    if config.improve.enabled { "on" } else { "off" },
    project.map_or(0, |p| p.runs),
    config.improve.every_runs,
    project
      .and_then(|p| p.latest.as_ref())
      .map(|p| p.error.clone().unwrap_or_else(|| p
        .experiment
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "no change proposed".into())))
      .unwrap_or_else(|| "no experiments yet".into())
  ))
}

pub(crate) fn experiment(workspace: &Path, id: Option<Uuid>) -> Result<PathBuf> {
  if let Some(id) = id {
    let store = AutomaticStore::default_store()?;
    let manual = store
      .root
      .parent()
      .context("no experiment root")?
      .join(id.to_string());
    if manual.exists() {
      return Ok(manual);
    }
    for entry in std::fs::read_dir(&store.root)? {
      let path = entry?.path().join(id.to_string());
      if path.join("report.json").exists() {
        return Ok(path);
      }
    }
    anyhow::bail!("no experiment named {id}");
  }
  AutomaticStore::default_store()?
    .read()?
    .projects
    .get(workspace)
    .and_then(|p| p.latest.as_ref())
    .and_then(|r| r.experiment.clone())
    .context("no experiment to review yet")
}

pub(crate) async fn chat(workspace: &Path, config: &mut Config, action: &str) -> Result<String> {
  match action {
    "on" | "off" => {
      config.improve.enabled = action == "on";
      config.save().await?;
      status(workspace, config)
    }
    "" | "review" => {
      let mut text = status(workspace, config)?;
      if let Ok(root) = experiment(workspace, None) {
        text.push_str("\n\n");
        text.push_str(&improve::review(&root).await?);
        mark_reviewed(workspace, &root).await?;
        text.push_str("\n\n/improve approve accepts this patch and its skills");
      }
      Ok(text)
    }
    "approve" => accept(&reviewed(workspace, None).await?, config).await,
    _ => anyhow::bail!("usage: /improve [on|off|approve]"),
  }
}

async fn accept(root: &Path, config: &Config) -> Result<String> {
  let report: improve::ImprovementReport =
    serde_json::from_slice(&tokio::fs::read(root.join("report.json")).await?)?;
  if report.automatic {
    ensure!(
      report
        .baseline
        .iter()
        .map(|check| &check.command)
        .eq(config.improve.checks.iter()),
      "the acceptance checks changed since evaluation; run a new experiment"
    );
  }
  ainz::eval::EvalContext::load(&root.join("evaluation"))
    .await?
    .verify_runtime(config)?;
  improve::approve(root, &LocalSkills::new()?).await
}

async fn mark_reviewed(workspace: &Path, root: &Path) -> Result<()> {
  let hash = improve::review_hash(root).await?;
  AutomaticStore::default_store()?.update(|state| {
    let project = state.projects.entry(workspace.to_path_buf()).or_default();
    project.reviewed = Some(root.to_path_buf());
    project.reviewed_hash = Some(hash);
    Ok(())
  })
}

async fn reviewed(workspace: &Path, id: Option<Uuid>) -> Result<PathBuf> {
  if id.is_some() {
    return experiment(workspace, id);
  }
  let state = AutomaticStore::default_store()?.read()?;
  let project = state
    .projects
    .get(workspace)
    .context("review an improvement first")?;
  let root = project
    .reviewed
    .clone()
    .context("review the improvement first with /improve or ainz improve review")?;
  ensure!(
    Some(improve::review_hash(&root).await?) == project.reviewed_hash,
    "the reviewed result changed; review it again before accepting"
  );
  Ok(root)
}
