use std::{path::Path, process::Stdio, time::Duration};

// dependencies
use ainz::{
  Config, EventSink, PermissionMode, Session, deny_all,
  improve::{
    Improvement,
    automatic::{Active, AutomaticStore, Outcome},
  },
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::{fs, process::Command};

// project
use super::{ImproveArgs, perform::perform};

#[derive(Serialize, Deserialize)]
struct Request {
  job: Active,
  config: Config,
}

pub(crate) async fn after_save(session: &Session, config: &Config) -> Option<String> {
  if !config.improve.enabled || session.parent_id.is_some() {
    return None;
  }
  match schedule(session, config).await {
    Ok(notice) => notice,
    Err(error) => Some(format!(
      "automatic improvement was not scheduled: {error:#}"
    )),
  }
}

async fn schedule(session: &Session, config: &Config) -> Result<Option<String>> {
  let store = AutomaticStore::default_store()?;
  let notice = store.notice(&session.workspace)?;
  let Some(job) = store.note(session, &config.improve)? else {
    return Ok(notice);
  };
  let launched = async {
    let request = store.root.join(format!("{}.json", job.id));
    fs::write(
      &request,
      serde_json::to_vec(&Request {
        job: job.clone(),
        config: config.clone(),
      })?,
    )
    .await?;
    let log = fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(store.root.join(format!("{}.log", job.id)))
      .await?
      .into_std()
      .await;
    let mut child = Command::new(std::env::current_exe()?)
      .arg("--workspace")
      .arg(&session.workspace)
      .args(["improve", "worker"])
      .arg(&request)
      .current_dir(&session.workspace)
      .stdin(Stdio::null())
      .stdout(Stdio::from(log.try_clone()?))
      .stderr(Stdio::from(log))
      .process_group(0)
      .spawn()?;
    store.started(
      job.id,
      child
        .id()
        .ok_or_else(|| anyhow::anyhow!("worker has no process id"))?,
    )?;
    tokio::spawn(async move {
      let _ = child.wait().await;
    });
    Ok::<_, anyhow::Error>(())
  }
  .await;
  if let Err(error) = launched {
    store.finished(
      &job,
      Outcome {
        id: job.id,
        experiment: None,
        error: Some(format!("{error:#}")),
      },
    )?;
    return Err(error);
  }
  Ok(notice.or_else(|| Some("an improvement experiment is running in the background".into())))
}

pub(super) async fn worker(current: &Config, path: &Path) -> Result<()> {
  let request: Request = serde_json::from_slice(&fs::read(path).await?)?;
  let store = AutomaticStore::default_store()?;
  let job = request.job;
  ensure!(
    store
      .read()?
      .active
      .as_ref()
      .is_some_and(|a| a.id == job.id),
    "improvement request is no longer current"
  );
  if !current.improve.enabled {
    store.finished(
      &job,
      Outcome {
        id: job.id,
        experiment: None,
        error: Some("automatic improvement was turned off".into()),
      },
    )?;
    fs::remove_file(path).await?;
    return Ok(());
  }
  store.started(job.id, std::process::id())?;
  let mut config = request.config;
  config.permissions = PermissionMode::Auto;
  config.yeet = false;
  config.max_steps = config.max_steps.min(config.improve.max_steps);
  config.budget_usd = Some(
    config
      .budget_usd
      .unwrap_or(config.improve.budget_usd)
      .min(config.improve.budget_usd),
  );
  config.validate()?;
  let mut args = ImproveArgs::automatic();
  let artifacts = store.root.join(job.id.to_string());
  args.out = Some(artifacts.clone());
  let result = tokio::time::timeout(
    Duration::from_secs(config.improve.max_seconds),
    perform(
      &job.workspace,
      &config,
      &args,
      EventSink::default(),
      deny_all(),
    ),
  )
  .await;
  let outcome = match result {
    Ok(Ok(Some(mut experiment))) => {
      experiment.report.automatic = true;
      experiment.save().await?;
      Outcome {
        id: job.id,
        experiment: Some(experiment.root),
        error: if !experiment.report.changed_evaluations.is_empty() {
          Some("candidate changed existing acceptance tests or check scripts".into())
        } else {
          (!experiment.report.acceptable()).then(|| {
            if let Err(error) = experiment.report.semantic_status() {
              return format!("{error:#}");
            }
            experiment
              .report
              .agent_error
              .unwrap_or_else(|| "candidate verification failed".into())
          })
        },
      }
    }
    Ok(Ok(None)) => Outcome {
      id: job.id,
      experiment: None,
      error: Some("no saved run evidence was available".into()),
    },
    Ok(Err(error)) => Outcome {
      id: job.id,
      experiment: None,
      error: Some(format!("{error:#}")),
    },
    Err(_) => Outcome {
      id: job.id,
      experiment: None,
      error: Some("experiment reached its time limit".into()),
    },
  };
  store.finished(&job, outcome)?;
  // build products and disposable workspaces can dwarf the evidence being kept
  if let Ok(mut entries) = fs::read_dir(&artifacts).await {
    while let Some(entry) = entries.next_entry().await? {
      for directory in ["build", "before", "after", "candidate"] {
        let _ = fs::remove_dir_all(entry.path().join(directory)).await;
      }
    }
  }
  fs::remove_file(path).await?;
  Ok(())
}

pub(super) fn remember_result(workspace: &Path, experiment: &Improvement) -> Result<()> {
  AutomaticStore::default_store()?.update(|state| {
    state
      .projects
      .entry(workspace.to_path_buf())
      .or_default()
      .latest = Some(Outcome {
      id: experiment.report.id,
      experiment: Some(experiment.root.clone()),
      error: (!experiment.report.acceptable()).then(|| {
        if let Err(error) = experiment.report.semantic_status() {
          return format!("{error:#}");
        }
        experiment
          .report
          .agent_error
          .clone()
          .unwrap_or_else(|| "candidate verification failed".into())
      }),
    });
    Ok(())
  })
}
