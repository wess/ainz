use std::{
  io::SeekFrom,
  path::{Path, PathBuf},
  process::Stdio,
  sync::Arc,
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
  fs,
  io::{AsyncReadExt, AsyncSeekExt},
  process::Command,
};
use uuid::Uuid;

use crate::{
  process::kill_group,
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

// $0 carries the job id through the ps command line so a reused pid is never mistaken for ours
const RUNNER: &str = r#"
sh -c "$2"
code=$?
printf '%s\n' "$code" > "$1.tmp"
mv "$1.tmp" "$1"
exit "$code"
"#;

#[derive(Clone, Debug)]
pub struct JobStore {
  root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct JobRecord {
  id: Uuid,
  workspace: PathBuf,
  command: String,
  pid: u32,
  started_at: u64,
}

#[derive(Debug, Serialize)]
struct JobStatus<'a> {
  id: Uuid,
  command: &'a str,
  pid: u32,
  started_at: u64,
  state: &'static str,
  #[serde(skip_serializing_if = "Option::is_none")]
  exit_code: Option<i32>,
}

impl JobStore {
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn default_path() -> Result<PathBuf> {
    Ok(
      dirs::data_local_dir()
        .context("could not locate the data directory")?
        .join("ainz/jobs"),
    )
  }

  pub fn default_store() -> Result<Self> {
    Ok(Self::new(Self::default_path()?))
  }

  pub fn tool(self) -> Arc<dyn Tool> {
    Arc::new(JobTool { store: self })
  }

  async fn start(&self, workspace: &Path, command: String) -> Result<JobRecord> {
    let id = Uuid::now_v7();
    let directory = self.root.join(id.to_string());
    fs::create_dir_all(&directory).await?;
    let output_path = directory.join("output.log");
    let status_path = directory.join("exit");
    let output = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&output_path)?;
    let error = output.try_clone()?;
    let status = status_path.to_string_lossy().into_owned();
    let child = Command::new("sh")
      .args(["-c", RUNNER, "ainz-job", &status, &command])
      .current_dir(workspace)
      .stdin(Stdio::null())
      .stdout(Stdio::from(output))
      .stderr(Stdio::from(error))
      .kill_on_drop(false)
      .process_group(0)
      .spawn()
      .context("start background job")?;
    let pid = child.id().context("background job had no process id")?;
    drop(child);
    let record = JobRecord {
      id,
      workspace: workspace.to_path_buf(),
      command,
      pid,
      started_at: now(),
    };
    self.save(&record).await?;
    Ok(record)
  }

  async fn save(&self, record: &JobRecord) -> Result<()> {
    let path = self.root.join(record.id.to_string()).join("job.json");
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(record)?).await?;
    fs::rename(temporary, path).await?;
    Ok(())
  }

  async fn load(&self, id: Uuid, workspace: &Path) -> Result<JobRecord> {
    let path = self.root.join(id.to_string()).join("job.json");
    let record: JobRecord = serde_json::from_slice(
      &fs::read(&path)
        .await
        .with_context(|| format!("read job {id}"))?,
    )?;
    if record.workspace != workspace {
      bail!("job {id} does not belong to this workspace");
    }
    Ok(record)
  }

  async fn list(&self, workspace: &Path) -> Result<Vec<JobRecord>> {
    let mut records = Vec::new();
    let mut entries = match fs::read_dir(&self.root).await {
      Ok(entries) => entries,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
      Err(error) => return Err(error).context("read job directory"),
    };
    while let Some(entry) = entries.next_entry().await? {
      let path = entry.path().join("job.json");
      if let Ok(bytes) = fs::read(path).await
        && let Ok(record) = serde_json::from_slice::<JobRecord>(&bytes)
        && record.workspace == workspace
      {
        records.push(record);
      }
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.started_at));
    Ok(records)
  }

  async fn status<'a>(&self, record: &'a JobRecord) -> Result<JobStatus<'a>> {
    let directory = self.root.join(record.id.to_string());
    if fs::try_exists(directory.join("stopped")).await? {
      return Ok(status(record, "stopped", None));
    }
    if let Ok(value) = fs::read_to_string(directory.join("exit")).await {
      let code = value.trim().parse().context("invalid job exit status")?;
      return Ok(status(record, "exited", Some(code)));
    }
    if self.owns_process(record).await? {
      Ok(status(record, "running", None))
    } else {
      Ok(status(record, "lost", None))
    }
  }

  async fn owns_process(&self, record: &JobRecord) -> Result<bool> {
    let output = Command::new("ps")
      .args(["-ww", "-o", "command=", "-p", &record.pid.to_string()])
      .output()
      .await
      .context("inspect background job")?;
    Ok(
      output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains(&record.id.to_string()),
    )
  }

  async fn stop(&self, record: &JobRecord) -> Result<()> {
    if !self.owns_process(record).await? {
      bail!("job {} is not running", record.id);
    }
    kill_group(record.pid, libc::SIGTERM).with_context(|| format!("stop job {}", record.id))?;
    fs::write(self.root.join(record.id.to_string()).join("stopped"), b"").await?;
    Ok(())
  }
}

fn status<'a>(record: &'a JobRecord, state: &'static str, exit_code: Option<i32>) -> JobStatus<'a> {
  JobStatus {
    id: record.id,
    command: &record.command,
    pid: record.pid,
    started_at: record.started_at,
    state,
    exit_code,
  }
}

struct JobTool {
  store: JobStore,
}

#[async_trait]
impl Tool for JobTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "job".into(),
      description: "Start and manage durable background shell jobs".into(),
      parameters: json!({
        "type": "object", "properties": {
          "command": {"type": "string", "enum": ["start", "list", "status", "output", "stop"]},
          "shell": {"type": "string"},
          "id": {"type": "string", "format": "uuid"}
        }, "required": ["command"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, arguments: &Value) -> Risk {
    match arguments.get("command").and_then(Value::as_str) {
      Some("start" | "stop") => Risk::Execute,
      _ => Risk::Read,
    }
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let command = arguments
      .get("command")
      .and_then(Value::as_str)
      .context("command is required")?;
    let output = match command {
      "start" => {
        let shell = arguments
          .get("shell")
          .and_then(Value::as_str)
          .context("shell is required")?;
        let record = self.store.start(&context.workspace, shell.into()).await?;
        serde_json::to_string_pretty(&self.store.status(&record).await?)?
      }
      "list" => {
        let records = self.store.list(&context.workspace).await?;
        let mut statuses = Vec::with_capacity(records.len());
        for record in &records {
          statuses.push(self.store.status(record).await?);
        }
        serde_json::to_string_pretty(&statuses)?
      }
      "status" => {
        let record = self.record(&arguments, context).await?;
        serde_json::to_string_pretty(&self.store.status(&record).await?)?
      }
      "output" => {
        let record = self.record(&arguments, context).await?;
        let path = self
          .store
          .root
          .join(record.id.to_string())
          .join("output.log");
        tail(&path, context.max_output_bytes).await?
      }
      "stop" => {
        let record = self.record(&arguments, context).await?;
        self.store.stop(&record).await?;
        serde_json::to_string_pretty(&self.store.status(&record).await?)?
      }
      _ => bail!("unknown job command: {command}"),
    };
    Ok(truncate(output, context.max_output_bytes))
  }
}

impl JobTool {
  async fn record(&self, arguments: &Value, context: &ToolContext) -> Result<JobRecord> {
    let id = arguments
      .get("id")
      .and_then(Value::as_str)
      .context("id is required")?
      .parse()
      .context("id must be a UUID")?;
    self.store.load(id, &context.workspace).await
  }
}

const OMITTED: &str = "[earlier output omitted]\n";

// the end of a log is where the failure is, so a long log keeps its tail rather than its head.
// the result fits `budget` with the marker included, so the caller's truncate never clips it
async fn tail(path: &Path, budget: usize) -> Result<String> {
  let mut file = fs::File::open(path)
    .await
    .with_context(|| format!("read {}", path.display()))?;
  let length = file.metadata().await?.len();
  let mut bytes = Vec::new();
  if usize::try_from(length).is_ok_and(|length| length <= budget) {
    file.read_to_end(&mut bytes).await?;
    return Ok(String::from_utf8_lossy(&bytes).into_owned());
  }
  let limit = budget.saturating_sub(OMITTED.len());
  file
    .seek(SeekFrom::End(-i64::try_from(limit).unwrap_or(i64::MAX)))
    .await?;
  file.read_to_end(&mut bytes).await?;
  let mut text = String::from_utf8_lossy(&bytes).into_owned();
  if let Some(newline) = text.find('\n') {
    text.drain(..=newline);
  }
  Ok(format!("{OMITTED}{text}"))
}

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
