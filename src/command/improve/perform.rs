use std::path::Path;

// dependencies
use ainz::{
  Agent, Approver, Config, EventSink, LocalSkills, RunOptions, RuntimeProvider, Session,
  SessionStore, SkillCatalog, Teacher,
  improve::{Improvement, evidence},
  instruction,
  protocol::Role,
  tool::{ToolSet, builtins},
};
use anyhow::{Context, Result, ensure};

// project
use super::{ImproveArgs, source};

pub(super) async fn perform(
  workspace: &Path,
  config: &Config,
  args: &ImproveArgs,
  events: EventSink,
  approver: Approver,
) -> Result<Option<Improvement>> {
  let evidence = evidence(
    &SessionStore::default_store()?,
    workspace,
    args.sessions.into(),
  )
  .await?;
  if evidence.sessions.is_empty() && args.focus.is_none() {
    return Ok(None);
  }
  let source = source::resolve(
    workspace,
    args.source.as_deref().or(config.improve.source.as_deref()),
  )
  .await?;
  let out = match &args.out {
    Some(path) => path.clone(),
    None => dirs::data_local_dir()
      .context("could not locate the data directory")?
      .join("ainz/improvements"),
  };
  let mut experiment =
    Improvement::prepare(&source, workspace, &out, evidence, args.focus.clone()).await?;
  experiment.report.model = Some(config.model.clone());
  experiment.report.provider = config.provider.clone();
  experiment.save().await?;
  let checks = if args.checks.is_empty() {
    &config.improve.checks
  } else {
    &args.checks
  };
  experiment.baseline(checks, args.timeout.into()).await?;
  let context = experiment.baseline_behavior(config).await?;
  if let Some(error) = experiment
    .report
    .behavior
    .as_ref()
    .and_then(|b| b.baseline.error.clone())
  {
    experiment.report.agent_error = Some(format!(
      "baseline behavioral evaluation was unavailable: {error}"
    ));
    experiment.save().await?;
    return Ok(Some(experiment));
  }
  let remaining =
    context.config.budget_usd.unwrap_or(0.5) - experiment.report.usage.cost_usd.unwrap_or(0.0);
  ensure!(
    remaining > 0.0,
    "baseline evaluation exhausted the experiment budget"
  );
  let mut run_config = config.clone();
  run_config.budget_usd = Some(remaining);
  let candidate = experiment.candidate();
  let provider = RuntimeProvider::configured(&run_config, &candidate).await?;
  let mut tools = ToolSet::default();
  tools.extend(builtins())?;
  tools.insert(ainz::todo::tool())?;
  tools.insert(
    SkillCatalog::discover(&experiment.report.source)
      .await?
      .tool(),
  )?;
  tools.insert(Teacher::Local(LocalSkills::with_root(experiment.root.join("skills"))).tool())?;
  let agent = Agent::new(provider, tools, candidate.clone(), events, approver);
  let mut session = Session::new(candidate.clone());
  let options = RunOptions {
    instructions: instruction::load(&candidate).await?,
    hooks: Default::default(),
    ..RunOptions::from(&run_config)
  };
  let result = agent.run(&mut session, experiment.prompt(), options).await;
  let result = result.and_then(|text| {
    ensure!(
      session
        .messages()?
        .last()
        .is_some_and(|message| message.role == Role::Assistant && message.tool_calls.is_empty()),
      "improvement paused with work still open"
    );
    Ok(text)
  });
  ainz::eval::add_usage(&mut experiment.report.usage, &session.usage);
  tokio::fs::write(
    experiment.root.join("session.json"),
    serde_json::to_vec_pretty(&session)?,
  )
  .await?;
  experiment.finish(result, args.timeout.into()).await?;
  if experiment.report.checks_passed && experiment.report.changed_evaluations.is_empty() {
    experiment.candidate_behavior(&context).await?;
  }
  Ok(Some(experiment))
}
