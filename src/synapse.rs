use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
  config::{Config, MemoryBackend, SynapseConfig},
  mcp::{McpHub, McpProfile, McpServerConfig, McpTransport},
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

/// Connect Ainz's native integration and its visible MCP profile in one operation.
///
/// Keeping both records together matters because Synapse reads the MCP profile to report whether
/// Ainz is connected, while Ainz reads its own setting to enable shared memory and guidance.
pub async fn connect(
  config: &mut Config,
  command: Option<PathBuf>,
  shared_memory: bool,
) -> Result<PathBuf> {
  let original_config = config.clone();
  if let Some(command) = command {
    config.synapse.command = Some(command.display().to_string());
  }
  let command = binary(&config.synapse).context(format!(
    "Synapse is not installed — install it from {SITE} or pass --command"
  ))?;
  config.synapse.command = Some(command.display().to_string());
  config.synapse.enabled = true;
  if shared_memory {
    config.memory.backend = MemoryBackend::Synapse;
  }

  let mut profile = McpProfile::load().await?;
  let original_profile = profile.clone();
  if let Some(server) = profile.servers.get(SERVER)
    && !is_profile_server(server)
  {
    *config = original_config;
    bail!(
      "the MCP profile has a non-Synapse server named `synapse`; rename or remove it before \
       connecting"
    );
  }
  let mut server = server_config(&command, Path::new("."));
  // A persistent profile follows the workspace Ainz was launched in. The native integration
  // loads Synapse's guidance itself, so a stopped Synapse must not prevent Ainz from starting.
  server.cwd = None;
  profile.servers.insert(SERVER.into(), server);
  if let Err(error) = profile.save().await {
    *config = original_config;
    return Err(error).context("save the Ainz MCP profile");
  }
  if let Err(error) = config.save().await {
    *config = original_config;
    original_profile
      .save()
      .await
      .context("restore the MCP profile after the Ainz configuration could not be saved")?;
    return Err(error).context("save the Ainz Synapse connection");
  }
  Ok(command)
}

/// Remove only the reciprocal Ainz/Synapse connection and return memory to the local backend.
pub async fn disconnect(config: &mut Config) -> Result<()> {
  let original_config = config.clone();
  let mut profile = McpProfile::load().await?;
  let original_profile = profile.clone();
  if let Some(server) = profile.servers.get(SERVER) {
    if !is_profile_server(server) {
      bail!(
        "the MCP profile has a non-Synapse server named `synapse`; remove it explicitly with \
         `ainz mcp remove synapse`"
      );
    }
  }
  profile.servers.remove(SERVER);
  profile.save().await?;

  config.synapse.enabled = false;
  config.synapse.mesh = false;
  config.synapse.command = None;
  if config.memory.backend == MemoryBackend::Synapse {
    config.memory.backend = MemoryBackend::Local;
  }
  if let Err(error) = config.save().await {
    *config = original_config;
    original_profile
      .save()
      .await
      .context("restore the MCP profile after the Ainz configuration could not be saved")?;
    return Err(error).context("save the disconnected Ainz configuration");
  }
  Ok(())
}

/// Whether Ainz's MCP profile exposes the reciprocal connection Synapse detects.
pub async fn profile_connected() -> Result<bool> {
  let profile = McpProfile::load().await?;
  Ok(profile.servers.get(SERVER).is_some_and(is_profile_server))
}

fn is_profile_server(server: &McpServerConfig) -> bool {
  server.transport == McpTransport::Stdio && server.command.len() == 2 && server.command[1] == "mcp"
}

/// One secret Synapse holds: its qualified name, the env var it resolves into, and whether it
/// is readable outside an approved directory scope.
pub struct Secret {
  pub name: String,
  pub var: String,
  pub global: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MeshAgent {
  pub name: String,
  pub role: String,
  pub status: String,
  pub note: String,
  pub project: String,
  pub tool: String,
  pub human: bool,
  pub registered: bool,
  pub online: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MeshWorker {
  pub name: String,
  pub role: String,
  pub status: String,
  pub process: u32,
  pub restarts: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MeshRoster {
  pub enabled: bool,
  pub agents: Vec<MeshAgent>,
  pub workers: Vec<MeshWorker>,
}

/// Read the whole machine roster without joining it. The TUI uses the command surface here so
/// opening its sidebar does not create a second mesh seat or keep another MCP server alive.
pub async fn mesh_roster(config: &SynapseConfig) -> Result<MeshRoster> {
  let command = binary(config).context("Synapse is not installed")?;
  let output = Command::new(command)
    .args(["relay", "status", "--json"])
    .output()
    .await
    .context("could not read the Synapse mesh")?;
  if !output.status.success() {
    let message = String::from_utf8_lossy(&output.stderr);
    bail!("could not read the Synapse mesh: {}", message.trim());
  }
  serde_json::from_slice(&output.stdout).context("Synapse returned an invalid mesh roster")
}

/// Every secret Synapse holds, for a chooser to offer during provider setup. Empty whenever
/// Synapse can't answer — no binary, a bad exit, output that doesn't parse — because setup uses
/// this only to decide whether to show a picker; a broken or absent Synapse must degrade to "no
/// secrets offered", never to a failed setup.
pub async fn secrets(config: &SynapseConfig) -> Vec<Secret> {
  let Some(command) = binary(config) else {
    return Vec::new();
  };
  let Some(vaults) = run(&command, &["vault", "list"]).await else {
    return Vec::new();
  };
  let mut found = Vec::new();
  for vault in vaults
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
  {
    // one vault failing to list (deleted mid-walk, denied scope) shouldn't hide the rest
    if let Some(output) = run(&command, &["secret", "list", vault]).await {
      found.extend(parse_secrets(&output));
    }
  }
  found.sort_by(|a, b| a.name.cmp(&b.name));
  found
}

async fn run(command: &Path, args: &[&str]) -> Option<String> {
  let output = Command::new(command).args(args).output().await.ok()?;
  output
    .status
    .success()
    .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// parse `secret list` rows: qualified name, env var, scope word — tab-separated. rows with
/// the wrong shape are skipped rather than guessed at. public so tests/synapse.rs can exercise
/// it without a real vault.
pub fn parse_secrets(vault_output: &str) -> Vec<Secret> {
  vault_output
    .lines()
    .filter_map(|line| {
      let mut columns = line.split('\t');
      let name = columns.next()?.trim();
      let var = columns.next()?.trim();
      let scope = columns.next()?.trim();
      if name.is_empty() || var.is_empty() || columns.next().is_some() {
        return None;
      }
      let global = match scope {
        "global" => true,
        "scoped" => false,
        _ => return None,
      };
      Some(Secret {
        name: name.to_string(),
        var: var.to_string(),
        global,
      })
    })
    .collect()
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
