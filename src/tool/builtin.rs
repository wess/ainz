use std::{process::Stdio, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{fs, process::Command, time::timeout};

use super::{Risk, Tool, ToolContext, truncate};
use crate::protocol::ToolSpec;
use crate::workspace;

pub fn builtins() -> Vec<Arc<dyn Tool>> {
  ["read", "list", "search", "write", "edit", "shell"]
    .into_iter()
    .map(|name| Arc::new(Builtin { name }) as Arc<dyn Tool>)
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

async fn read(context: &ToolContext, value: Value) -> Result<String> {
  let args: PathArgs = serde_json::from_value(value)?;
  let path = workspace::existing(&context.workspace, &args.path).await?;
  let text = fs::read_to_string(&path)
    .await
    .with_context(|| format!("read {}", path.display()))?;
  let offset = args.offset.unwrap_or(1).saturating_sub(1);
  let limit = args.limit.unwrap_or(2_000);
  let output = text
    .lines()
    .skip(offset)
    .take(limit)
    .collect::<Vec<_>>()
    .join("\n");
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
  let output = Command::new("rg")
    .args([
      "--line-number",
      "--color",
      "never",
      "--max-count",
      &max_results,
    ])
    .arg(&args.query)
    .arg(path)
    .current_dir(&context.workspace)
    .output()
    .await
    .context("run rg")?;
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
  fs::write(&path, args.content.as_bytes()).await?;
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
  let path = workspace::existing(&context.workspace, &args.path).await?;
  let text = fs::read_to_string(&path).await?;
  let count = text.matches(&args.old).count();
  if count != 1 {
    bail!("expected one match in {}, found {count}", args.path);
  }
  fs::write(&path, text.replacen(&args.old, &args.new, 1)).await?;
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
  let child = Command::new("sh")
    .args(["-lc", &args.command])
    .current_dir(&context.workspace)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .spawn()?;
  let output = timeout(
    Duration::from_millis(args.timeout_ms),
    child.wait_with_output(),
  )
  .await
  .context("command timed out")??;
  let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
  text.push_str(&String::from_utf8_lossy(&output.stderr));
  text.push_str(&format!("\n[exit {}]", output.status.code().unwrap_or(-1)));
  Ok(truncate(text, context.max_output_bytes))
}
