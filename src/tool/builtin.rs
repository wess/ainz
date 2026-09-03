use std::{process::Stdio, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
  fs,
  io::{AsyncBufReadExt, BufReader},
  process::Command,
  sync::mpsc,
  time::timeout,
};

use super::{Risk, Tool, ToolContext, truncate};
use crate::{process::GroupGuard, protocol::ToolSpec, workspace};

pub fn builtins() -> Vec<Arc<dyn Tool>> {
  ["read", "list", "search", "write", "edit", "shell"]
    .into_iter()
    .map(|name| Arc::new(Builtin { name }) as Arc<dyn Tool>)
    .chain([super::fetch::tool()])
    .collect()
}

struct Builtin {
  name: &'static str,
}

#[async_trait]
impl Tool for Builtin {
  fn spec(&self) -> ToolSpec {
    match self.name {
      "read" => spec(
        "read",
        "Read a UTF-8 file from the workspace",
        json!({
          "type": "object", "properties": {
            "path": {"type": "string"}, "offset": {"type": "integer", "minimum": 1},
            "limit": {"type": "integer", "minimum": 1}
          }, "required": ["path"], "additionalProperties": false
        }),
      ),
      "list" => spec(
        "list",
        "List files and directories in a workspace directory",
        json!({
          "type": "object", "properties": {"path": {"type": "string"}},
          "additionalProperties": false
        }),
      ),
      "search" => spec(
        "search",
        "Search workspace text with a regular expression",
        json!({
          "type": "object", "properties": {
            "query": {"type": "string"}, "path": {"type": "string"},
            "max_results": {"type": "integer", "minimum": 1, "maximum": 500}
          }, "required": ["query"], "additionalProperties": false
        }),
      ),
      "write" => spec(
        "write",
        "Create or replace a UTF-8 file in the workspace",
        json!({
          "type": "object", "properties": {
            "path": {"type": "string"}, "content": {"type": "string"}
          }, "required": ["path", "content"], "additionalProperties": false
        }),
      ),
      "edit" => spec(
        "edit",
        "Replace one exact text occurrence in a workspace file",
        json!({
          "type": "object", "properties": {
            "path": {"type": "string"}, "old": {"type": "string"},
            "new": {"type": "string"}
          }, "required": ["path", "old", "new"], "additionalProperties": false
        }),
      ),
      "shell" => spec(
        "shell",
        "Run a shell command in the workspace",
        json!({
          "type": "object", "properties": {
            "command": {"type": "string"},
            "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 300000}
          }, "required": ["command"], "additionalProperties": false
        }),
      ),
      _ => unreachable!(),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    match self.name {
      "read" | "list" | "search" => Risk::Read,
      "write" | "edit" => Risk::Write,
      "shell" => Risk::Execute,
      _ => unreachable!(),
    }
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    match self.name {
      "read" => read(context, arguments).await,
      "list" => list(context, arguments).await,
      "search" => search(context, arguments).await,
      "write" => write(context, arguments).await,
      "edit" => edit(context, arguments).await,
      "shell" => shell(context, arguments).await,
      _ => unreachable!(),
    }
  }
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
  ToolSpec {
    name: name.into(),
    description: description.into(),
    parameters,
  }
}

#[derive(Deserialize)]
struct PathArgs {
  #[serde(default = "dot")]
  path: String,
  offset: Option<usize>,
  limit: Option<usize>,
}

fn dot() -> String {
  ".".into()
}

// streams lines so a large file costs only the requested window, not the whole file
async fn read(context: &ToolContext, value: Value) -> Result<String> {
  let args: PathArgs = serde_json::from_value(value)?;
  let path = workspace::existing(&context.workspace, &args.path).await?;
  let file = fs::File::open(&path)
    .await
    .with_context(|| format!("read {}", path.display()))?;
  let mut lines = BufReader::new(file).lines();
  let offset = args.offset.unwrap_or(1).saturating_sub(1);
  let limit = args.limit.unwrap_or(2_000);
  let mut output = String::new();
  let mut index = 0;
  let mut taken = 0;
  while let Some(line) = lines
    .next_line()
    .await
    .with_context(|| format!("read {}", path.display()))?
  {
    if index >= offset {
      if taken == limit || output.len() > context.max_output_bytes {
        break;
      }
      if taken > 0 {
        output.push('\n');
      }
      output.push_str(&line);
      taken += 1;
    }
    index += 1;
  }
  Ok(truncate(output, context.max_output_bytes))
}

async fn list(context: &ToolContext, value: Value) -> Result<String> {
  let args: PathArgs = serde_json::from_value(value)?;
  let path = workspace::existing(&context.workspace, &args.path).await?;
  let mut entries = fs::read_dir(&path)
    .await
    .with_context(|| format!("list {}", path.display()))?;
  let mut names = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    let suffix = if entry.file_type().await?.is_dir() {
      "/"
    } else {
      ""
    };
    names.push(format!("{}{suffix}", entry.file_name().to_string_lossy()));
  }
  names.sort();
  Ok(truncate(names.join("\n"), context.max_output_bytes))
}

#[derive(Deserialize)]
struct SearchArgs {
  query: String,
  #[serde(default = "dot")]
  path: String,
  #[serde(default = "default_results")]
  max_results: usize,
}

fn default_results() -> usize {
  100
}

async fn search(context: &ToolContext, value: Value) -> Result<String> {
  let args: SearchArgs = serde_json::from_value(value)?;
  let path = workspace::existing(&context.workspace, &args.path).await?;
  let max_results = args.max_results.to_string();
  // NB: the query goes through -e and the path after -- so neither can be read as an rg flag
  let output = Command::new("rg")
    .args(["--line-number", "--color", "never", "--max-count"])
    .arg(&max_results)
    .args(["-e", &args.query, "--"])
    .arg(path)
    .current_dir(&context.workspace)
    .stdin(Stdio::null())
    .output()
    .await
    .context("run rg (ripgrep must be installed)")?;
  if !output.status.success() && output.status.code() != Some(1) {
    bail!("rg failed: {}", String::from_utf8_lossy(&output.stderr));
  }
  Ok(truncate(
    String::from_utf8_lossy(&output.stdout).into_owned(),
    context.max_output_bytes,
  ))
}

#[derive(Deserialize)]
struct WriteArgs {
  path: String,
  content: String,
}

async fn write(context: &ToolContext, value: Value) -> Result<String> {
  let args: WriteArgs = serde_json::from_value(value)?;
  let path = workspace::writable(&context.workspace, &args.path).await?;
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).await?;
  }
  fs::write(&path, args.content.as_bytes())
    .await
    .with_context(|| format!("write {}", args.path))?;
  Ok(format!(
    "wrote {} bytes to {}",
    args.content.len(),
    args.path
  ))
}

#[derive(Deserialize)]
struct EditArgs {
  path: String,
  old: String,
  new: String,
}

async fn edit(context: &ToolContext, value: Value) -> Result<String> {
  let args: EditArgs = serde_json::from_value(value)?;
  if args.old.is_empty() {
    bail!("old text must not be empty");
  }
  let path = workspace::existing(&context.workspace, &args.path).await?;
  let text = fs::read_to_string(&path)
    .await
    .with_context(|| format!("read {}", args.path))?;
  let count = text.matches(&args.old).count();
  if count != 1 {
    bail!("expected one match in {}, found {count}", args.path);
  }
  fs::write(&path, text.replacen(&args.old, &args.new, 1))
    .await
    .with_context(|| format!("write {}", args.path))?;
  Ok(format!("edited {}", args.path))
}

#[derive(Deserialize)]
struct ShellArgs {
  command: String,
  #[serde(default = "default_timeout")]
  timeout_ms: u64,
}

fn default_timeout() -> u64 {
  30_000
}

async fn shell(context: &ToolContext, value: Value) -> Result<String> {
  let args: ShellArgs = serde_json::from_value(value)?;
  let mut child = Command::new("sh")
    .args(["-c", &args.command])
    .current_dir(&context.workspace)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .process_group(0)
    .spawn()
    .context("start shell")?;
  let guard = GroupGuard::new(child.id());
  let (sender, mut lines) = mpsc::unbounded_channel();
  // both pipes are read as they fill, which is what makes a long command visible while it
  // runs; the cost is that stderr lands where it happened rather than after all of stdout
  for pipe in [
    child.stdout.take().map(Pipe::Out),
    child.stderr.take().map(Pipe::Err),
  ]
  .into_iter()
  .flatten()
  {
    let sender = sender.clone();
    tokio::spawn(async move {
      match pipe {
        Pipe::Out(pipe) => forward(BufReader::new(pipe).lines(), sender).await,
        Pipe::Err(pipe) => forward(BufReader::new(pipe).lines(), sender).await,
      }
    });
  }
  drop(sender);
  let drain = async {
    let mut text = String::new();
    while let Some(line) = lines.recv().await {
      context.report(&format!("{line}\n"));
      text.push_str(&line);
      text.push('\n');
    }
    text
  };
  let Ok((status, mut text)) = timeout(Duration::from_millis(args.timeout_ms), async {
    tokio::join!(child.wait(), drain)
  })
  .await
  else {
    bail!("command timed out after {} ms", args.timeout_ms);
  };
  let status = status.context("wait for shell")?;
  guard.disarm();
  text.push_str(&format!("[exit {}]", status.code().unwrap_or(-1)));
  Ok(truncate(text, context.max_output_bytes))
}

enum Pipe {
  Out(tokio::process::ChildStdout),
  Err(tokio::process::ChildStderr),
}

async fn forward<R: tokio::io::AsyncBufRead + Unpin>(
  mut lines: tokio::io::Lines<R>,
  sender: mpsc::UnboundedSender<String>,
) {
  while let Ok(Some(line)) = lines.next_line().await {
    if sender.send(line).is_err() {
      break;
    }
  }
}
