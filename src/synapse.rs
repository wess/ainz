use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
  config::SynapseConfig,
  mcp::{McpHub, McpServerConfig, McpTransport},
  memory::MemoryRecord,
};

// shown wherever the integration is offered, so the setting explains itself
pub const SITE: &str = "https://wess.io/synapse/";
pub const SUMMARY: &str =
  "Local memory, one skill library, and an agent mesh shared with your other tools.";
pub const SERVER: &str = "synapse";

const TIMEOUT_MS: u64 = 30_000;

/// The `synapse` executable, when the user has one. An override in the config wins; otherwise
/// PATH is searched, then the directories Synapse installs its CLI into.
pub fn binary(config: &SynapseConfig) -> Option<PathBuf> {
  if let Some(command) = config.command.as_deref().filter(|value| !value.is_empty()) {
    let path = PathBuf::from(command);
    if path.is_absolute() {
      return path.is_file().then_some(path);
    }
    return lookup(command);
  }
  lookup("synapse")
}

fn lookup(command: &str) -> Option<PathBuf> {
  let path = std::env::var_os("PATH").unwrap_or_default();
  let mut roots: Vec<PathBuf> = std::env::split_paths(&path).collect();
  if let Some(home) = dirs::home_dir() {
    roots.push(home.join(".local/bin"));
  }
  roots.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
  roots
    .into_iter()
    .map(|root| root.join(command))
    .find(|candidate| candidate.is_file())
}

/// The MCP server entry for a resolved binary. Optional rather than required: a Synapse that
/// will not start costs the session its memory, never its startup.
pub fn server_config(command: &Path, workspace: &Path) -> McpServerConfig {
  McpServerConfig {
    transport: McpTransport::Stdio,
    command: vec![command.display().to_string(), "mcp".into()],
    url: None,
    header_env: Default::default(),
    headers: Default::default(),
    env: Default::default(),
    cwd: Some(workspace.to_path_buf()),
    enabled: true,
    required: false,
    timeout_ms: TIMEOUT_MS,
  }
}

/// Typed access to the Synapse server already in the hub.
#[derive(Clone)]
pub struct Synapse {
  hub: Arc<McpHub>,
  project: PathBuf,
}

impl Synapse {
  pub fn new(hub: Arc<McpHub>, project: PathBuf) -> Self {
    Self { hub, project }
  }

  pub fn project(&self) -> &Path {
    &self.project
  }

  async fn call(&self, name: &str, mut arguments: Value) -> Result<String> {
    if let Some(object) = arguments.as_object_mut() {
      object.insert("project".into(), json!(self.project.display().to_string()));
    }
    let output = self.hub.call(SERVER, name, arguments).await?;
    match output.strip_prefix("[tool error] ") {
      Some(message) => bail!("synapse {name}: {}", message.trim()),
      None => Ok(output),
    }
  }

  /// SOUL.md and the rest of the server's own instructions, when it starts.
  pub async fn guidance(&self) -> Option<String> {
    self.hub.instructions_for(SERVER).await.ok().flatten()
  }

  pub async fn recall(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
    // recall wants words; "what is stored for this project" is the project's own name
    let query = match query.trim() {
      "" => self
        .project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into()),
      query => query.to_string(),
    };
    let output = self
      .call("recall", json!({"query": query, "limit": limit}))
      .await?;
    let value = first_object(&output).context("synapse recall returned no JSON object")?;
    let memories = value
      .get("memories")
      .and_then(Value::as_array)
      .cloned()
      .unwrap_or_default();
    Ok(memories.iter().map(record).collect())
  }

  pub async fn remember(
    &self,
    content: &str,
    source: Option<&str>,
    scope: &str,
    supersedes: &[String],
  ) -> Result<String> {
    let mut arguments = json!({"content": content, "scope": scope});
    if let Some(source) = source.filter(|value| !value.is_empty()) {
      arguments["source"] = json!(source);
    }
    if !supersedes.is_empty() {
      arguments["supersedes"] = json!(supersedes);
    }
    self.call("remember", arguments).await
  }

  pub async fn teach(
    &self,
    name: &str,
    description: &str,
    instructions: &str,
    scope: &str,
    note: Option<&str>,
  ) -> Result<String> {
    let mut arguments = json!({
      "name": name, "description": description, "instructions": instructions, "scope": scope
    });
    if let Some(note) = note.filter(|value| !value.is_empty()) {
      arguments["note"] = json!(note);
    }
    self.call("teach", arguments).await
  }

  pub async fn revise(
    &self,
    name: &str,
    instructions: &str,
    description: Option<&str>,
    note: Option<&str>,
  ) -> Result<String> {
    let mut arguments = json!({"name": name, "instructions": instructions});
    if let Some(description) = description.filter(|value| !value.is_empty()) {
      arguments["description"] = json!(description);
    }
    if let Some(note) = note.filter(|value| !value.is_empty()) {
      arguments["note"] = json!(note);
    }
    self.call("revise", arguments).await
  }

  /// Join the mesh. The roster comes back, so a caller can tell the session who else is here.
  pub async fn register(&self, name: &str, role: &str) -> Result<String> {
    self
      .call("register", json!({"name": name, "role": role}))
      .await
  }

  pub async fn report_status(&self, status: &str, note: Option<&str>) -> Result<String> {
    let mut arguments = json!({"status": status});
    if let Some(note) = note.filter(|value| !value.is_empty()) {
      arguments["note"] = json!(note);
    }
    self.call("reportstatus", arguments).await
  }
}

fn record(value: &Value) -> MemoryRecord {
  MemoryRecord {
    id: value
      .get("id")
      .map(|id| match id {
        Value::String(text) => text.clone(),
        other => other.to_string(),
      })
      .unwrap_or_default(),
    body: value
      .get("body")
      .and_then(Value::as_str)
      .unwrap_or_default()
      .to_string(),
    source: value
      .get("source")
      .and_then(Value::as_str)
      .map(str::to_string),
    scope: value
      .get("scope")
      .and_then(Value::as_str)
      .unwrap_or("project")
      .to_string(),
    created: value.get("created").and_then(Value::as_u64).unwrap_or(0),
  }
}

// tool output carries the text block and then any structured duplicate of it; the first line
// that parses is the answer
fn first_object(output: &str) -> Option<Value> {
  output
    .lines()
    .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
    .find(|value| value.is_object())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_the_first_json_line() {
    let output = "{\"memories\":[{\"id\":7,\"body\":\"hi\"}]}\n{\"memories\":[]}";
    let value = first_object(output).expect("object");
    let memories = value["memories"].as_array().expect("array");
    assert_eq!(record(&memories[0]).id, "7");
    assert_eq!(record(&memories[0]).body, "hi");
  }

  #[test]
  fn an_absolute_override_must_exist() {
    let config = SynapseConfig {
      enabled: true,
      mesh: false,
      command: Some("/nonexistent/synapse".into()),
    };
    assert!(binary(&config).is_none());
  }
}
