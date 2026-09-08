use std::{process::Stdio, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
  fs,
  io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
  process::Command,
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

  fn validate(&self, arguments: &Value) -> Result<()> {
    match self.name {
      "read" | "list" => {
        let _: PathArgs = serde_json::from_value(arguments.clone())?;
      }
      "search" => {
        let _: SearchArgs = serde_json::from_value(arguments.clone())?;
      }
      "write" => {
        let _: WriteArgs = serde_json::from_value(arguments.clone())?;
      }
      "edit" => {
        let _: EditArgs = serde_json::from_value(arguments.clone())?;
      }
      "shell" => {
        let _: super::shell::ShellArgs = serde_json::from_value(arguments.clone())?;
      }
      _ => unreachable!(),
    }
    Ok(())
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    match self.name {
      "read" => read(context, arguments).await,
      "list" => list(context, arguments).await,
      "search" => search(context, arguments).await,
      "write" => write(context, arguments).await,
      "edit" => edit(context, arguments).await,
      "shell" => super::shell::execute(context, arguments).await,
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
#[serde(deny_unknown_fields)]
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
  let file = workspace::open(&context.workspace, &args.path, false, false)
    .await
    .with_context(|| format!("read {}", path.display()))?;
  let mut reader = BufReader::new(file);
  let offset = args.offset.unwrap_or(1).saturating_sub(1);
  let limit = args.limit.unwrap_or(2_000);
  let mut output = String::new();
  let mut index = 0;
  let mut taken = 0;
  loop {
    let mut line = String::new();
    let read = (&mut reader)
      .take(
        context
          .max_output_bytes
          .min(16 * 1024 * 1024)
          .saturating_add(1) as u64,
      )
      .read_line(&mut line)
      .await
      .with_context(|| format!("read {}", path.display()))?;
    if read == 0 {
      break;
    }
    if !line.ends_with('\n') && read > context.max_output_bytes.min(16 * 1024 * 1024) {
      bail!("line exceeds the read transfer limit");
    }
    let line = line.trim_end_matches(['\r', '\n']);
    if index >= offset {
      if taken == limit || output.len() > context.max_output_bytes {
        break;
      }
      if taken > 0 {
        output.push('\n');
      }
      output.push_str(line);
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
  let mut size = 0;
  let mut truncated = false;
  while let Some(entry) = entries.next_entry().await? {
    let suffix = if entry.file_type().await?.is_dir() {
      "/"
    } else {
      ""
    };
    let name = format!("{}{suffix}", entry.file_name().to_string_lossy());
    size += name.len() + 1;
    if size > context.max_output_bytes {
      truncated = true;
      break;
    }
    names.push(name);
  }
  names.sort();
  let mut text = names.join("\n");
  if truncated {
    text.push_str("\n[output truncated]");
  }
  Ok(text)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
  let max_results = args.max_results.clamp(1, 500).to_string();
  // NB: the query goes through -e and the path after -- so neither can be read as an rg flag
  let mut child = Command::new("rg")
    .args(["--line-number", "--color", "never", "--max-count"])
    .arg(&max_results)
    .args(["-e", &args.query, "--"])
    .arg(path)
    .current_dir(&context.workspace)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .process_group(0)
    .spawn()
    .context("run rg (ripgrep must be installed)")?;
  let _guard = GroupGuard::new(child.id());
  let stdout = child.stdout.take().context("search stdout unavailable")?;
  let stderr = child.stderr.take().context("search stderr unavailable")?;
  let (status, output, error) = timeout(Duration::from_secs(30), async {
    tokio::try_join!(
      child.wait(),
      crate::output::capture(stdout, context.max_output_bytes),
      crate::output::capture(stderr, context.max_output_bytes)
    )
  })
  .await
  .context("search timed out")??;
  if !status.success() && status.code() != Some(1) {
    bail!("rg failed: {}", String::from_utf8_lossy(&error.bytes));
  }
  let mut text = String::from_utf8_lossy(&output.bytes).into_owned();
  if output.truncated {
    text.push_str("\n[output truncated]");
  }
  Ok(text)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
  path: String,
  content: String,
}

async fn write(context: &ToolContext, value: Value) -> Result<String> {
  let args: WriteArgs = serde_json::from_value(value)?;
  workspace::write(&context.workspace, &args.path, args.content.as_bytes()).await?;
  Ok(format!(
    "wrote {} bytes to {}",
    args.content.len(),
    args.path
  ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
  let mut file = workspace::open(&context.workspace, &args.path, true, false).await?;
  let mut text = String::new();
  (&mut file)
    .take(16 * 1024 * 1024 + 1)
    .read_to_string(&mut text)
    .await?;
  if text.len() > 16 * 1024 * 1024 {
    bail!("file exceeds the edit transfer limit");
  }
  let count = text.matches(&args.old).count();
  if count != 1 {
    bail!("expected one match in {}, found {count}", args.path);
  }
  file.rewind().await?;
  file.set_len(0).await?;
  file
    .write_all(text.replacen(&args.old, &args.new, 1).as_bytes())
    .await
    .with_context(|| format!("write {}", args.path))?;
  Ok(format!("edited {}", args.path))
}
