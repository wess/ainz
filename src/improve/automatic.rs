use std::{
  collections::BTreeMap,
  fs::{File, OpenOptions},
  os::{
    fd::AsRawFd,
    unix::fs::{OpenOptionsExt, PermissionsExt},
  },
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

// dependencies
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// project
use super::ImproveConfig;
use crate::Session;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Outcome {
  pub id: Uuid,
  pub experiment: Option<PathBuf>,
  pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Project {
  pub runs: usize,
  pub seen: Vec<Uuid>,
  pub last_attempt: Option<u64>,
  pub latest: Option<Outcome>,
  pub notified: Option<Uuid>,
  pub reviewed: Option<PathBuf>,
  pub reviewed_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Active {
  pub id: Uuid,
  pub workspace: PathBuf,
  pub pid: Option<u32>,
  pub started: u64,
  pub max_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AutomaticState {
  pub projects: BTreeMap<PathBuf, Project>,
  pub active: Option<Active>,
}

pub struct AutomaticStore {
  pub root: PathBuf,
}

impl AutomaticStore {
  pub fn default_store() -> Result<Self> {
    Ok(Self {
      root: dirs::data_local_dir()
        .context("could not locate the data directory")?
        .join("ainz/improvements/automatic"),
    })
  }

  pub fn update<T>(&self, f: impl FnOnce(&mut AutomaticState) -> Result<T>) -> Result<T> {
    std::fs::create_dir_all(&self.root)?;
    std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
    let lock = OpenOptions::new()
      .read(true)
      .write(true)
      .create(true)
      .truncate(false)
      .mode(0o600)
      .open(self.root.join("lock"))?;
    // no process holds this across model or check execution; a crashed owner releases it
    lock_file(&lock)?;
    let mut state = self.read()?;
    let result = f(&mut state)?;
    let path = self.root.join("state.json");
    let temp = self.root.join(format!("{}.tmp", Uuid::now_v7()));
    let write = (|| {
      use std::io::Write;
      let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
      file.write_all(&serde_json::to_vec(&state)?)?;
      file.sync_all()?;
      std::fs::rename(&temp, &path)?;
      Ok::<_, anyhow::Error>(())
    })();
    if write.is_err() {
      let _ = std::fs::remove_file(temp);
    }
    write?;
    Ok(result)
  }

  pub fn read(&self) -> Result<AutomaticState> {
    match std::fs::read(self.root.join("state.json")) {
      Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AutomaticState::default()),
      Err(error) => Err(error.into()),
    }
  }

  pub fn note(&self, session: &Session, policy: &ImproveConfig) -> Result<Option<Active>> {
    if !policy.enabled || session.parent_id.is_some() || session.cursor.is_none() {
      return Ok(None);
    }
    policy.validate()?;
    self.update(|state| {
      let now = now();
      if state.active.as_ref().is_some_and(|a| expired(a, now)) {
        state.active = None;
      }
      let project = state.projects.entry(session.workspace.clone()).or_default();
      let cursor = session.cursor.context("run has no cursor")?;
      if !project.seen.contains(&cursor) {
        project.seen.push(cursor);
        if project.seen.len() > 128 {
          project.seen.remove(0);
        }
        project.runs = project.runs.saturating_add(1);
      }
      if state.active.is_some()
        || project.runs < policy.every_runs
        || project
          .last_attempt
          .is_some_and(|last| now.saturating_sub(last) < policy.cooldown_seconds)
      {
        return Ok(None);
      }
      project.runs = 0;
      project.last_attempt = Some(now);
      let active = Active {
        id: Uuid::now_v7(),
        workspace: session.workspace.clone(),
        pid: None,
        started: now,
        max_seconds: policy.max_seconds,
      };
      state.active = Some(active.clone());
      Ok(Some(active))
    })
  }

  pub fn started(&self, id: Uuid, pid: u32) -> Result<()> {
    self.update(|state| {
      if let Some(active) = &mut state.active
        && active.id == id
      {
        active.pid = Some(pid);
      }
      Ok(())
    })
  }

  pub fn finished(&self, job: &Active, outcome: Outcome) -> Result<()> {
    self.update(|state| {
      if state
        .active
        .as_ref()
        .is_some_and(|active| active.id == job.id)
      {
        state.active = None;
        state
          .projects
          .entry(job.workspace.clone())
          .or_default()
          .latest = Some(outcome);
      }
      Ok(())
    })
  }

  pub fn notice(&self, workspace: &Path) -> Result<Option<String>> {
    let state = self.read()?;
    let Some(project) = state.projects.get(workspace) else {
      return Ok(None);
    };
    if project
      .latest
      .as_ref()
      .is_none_or(|latest| project.notified == Some(latest.id))
    {
      return Ok(None);
    }
    self.update(|state| {
      let Some(project) = state.projects.get_mut(workspace) else {
        return Ok(None);
      };
      let Some(latest) = &project.latest else {
        return Ok(None);
      };
      if project.notified == Some(latest.id) {
        return Ok(None);
      }
      project.notified = Some(latest.id);
      Ok(Some(match &latest.error {
        Some(error) => format!("automatic improvement stopped: {error}. /improve shows the result"),
        None => "a checked harness improvement is ready; /improve reviews it".into(),
      }))
    })
  }
}

fn lock_file(file: &File) -> Result<()> {
  // SAFETY: flock only operates on this live file descriptor
  if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
    return Err(std::io::Error::last_os_error()).context("lock improvement state");
  }
  Ok(())
}

fn expired(active: &Active, now: u64) -> bool {
  if now.saturating_sub(active.started) > active.max_seconds.saturating_add(120) {
    return true;
  }
  match active.pid {
    // SAFETY: signal zero only checks whether the process exists
    Some(pid) => {
      (unsafe { libc::kill(pid as i32, 0) }) != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    None => now.saturating_sub(active.started) > 30,
  }
}

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
