mod client;
mod tool;

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{Arc, Mutex as SyncMutex, PoisonError},
};

use anyhow::{Context, Result, bail};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
  fs,
  sync::{MappedMutexGuard, Mutex, MutexGuard},
};

use crate::tool::{Risk, Tool, truncate};

use self::{client::Client, tool::McpTool};

const MAX_INSTRUCTIONS: usize = 4 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct McpProfile {
  #[serde(default)]
  pub servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(from = "McpServerConfigFile", into = "McpServerConfigFile")]
pub struct McpServerConfig {
  pub transport: McpTransport,
  pub command: Vec<String>,
  pub url: Option<String>,
  pub header_env: BTreeMap<String, String>,
  pub headers: BTreeMap<String, String>,
  pub env: BTreeMap<String, String>,
  pub cwd: Option<PathBuf>,
  pub enabled: bool,
  pub required: bool,
  pub timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct McpServerConfigFile {
  #[serde(default)]
  transport: McpTransport,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  command: Option<CommandField>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  args: Vec<String>,
  url: Option<String>,
  #[serde(default)]
  header_env: BTreeMap<String, String>,
  #[serde(default)]
  headers: BTreeMap<String, String>,
  #[serde(default)]
  env: BTreeMap<String, String>,
  cwd: Option<PathBuf>,
  #[serde(default = "enabled")]
  enabled: bool,
  #[serde(default)]
  required: bool,
  #[serde(default = "default_timeout")]
  timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum CommandField {
  String(String),
  List(Vec<String>),
}

impl From<McpServerConfigFile> for McpServerConfig {
  fn from(file: McpServerConfigFile) -> Self {
    let command = match file.command {
      Some(CommandField::String(command)) => std::iter::once(command).chain(file.args).collect(),
      Some(CommandField::List(command)) => command.into_iter().chain(file.args).collect(),
      None => file.args,
    };
    Self {
      transport: file.transport,
      command,
      url: file.url,
      header_env: file.header_env,
      headers: file.headers,
      env: file.env,
      cwd: file.cwd,
      enabled: file.enabled,
      required: file.required,
      timeout_ms: file.timeout_ms,
    }
  }
}

impl From<McpServerConfig> for McpServerConfigFile {
  fn from(config: McpServerConfig) -> Self {
    let mut command = config.command.into_iter();
    Self {
      transport: config.transport,
      command: command.next().map(CommandField::String),
      args: command.collect(),
      url: config.url,
      header_env: config.header_env,
      headers: config.headers,
      env: config.env,
      cwd: config.cwd,
      enabled: config.enabled,
      required: config.required,
      timeout_ms: config.timeout_ms,
    }
  }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
  #[default]
  Stdio,
  StreamableHttp,
}

fn enabled() -> bool {
  true
}

fn default_timeout() -> u64 {
  30_000
}

// server names appear in tool descriptions and `server/tool` targets, so keep them plain
pub fn valid_name(name: &str) -> bool {
  !name.is_empty()
    && name.len() <= 64
    && name
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl McpProfile {
  pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AINZ_MCP_PROFILE") {
      return Ok(PathBuf::from(path));
    }
    Ok(
      dirs::config_dir()
        .context("could not locate the config directory")?
        .join("ainz/mcp.toml"),
    )
  }

  pub async fn load() -> Result<Self> {
    let path = Self::path()?;
    let legacy = dirs::config_dir()
      .context("could not locate the config directory")?
      .join("agentx/mcp.toml");
    let source =
      if !path.exists() && std::env::var_os("AINZ_MCP_PROFILE").is_none() && legacy.exists() {
        &legacy
      } else {
        &path
      };
    let profile: Self = match fs::read_to_string(source).await {
      Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", source.display())),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
      Err(error) => Err(error).with_context(|| format!("read {}", source.display())),
    }?;
    profile.validate()?;
    if source == &legacy {
      profile.save_to(&path).await?;
    }
    Ok(profile)
  }

  pub async fn load_with(extra: Option<&Path>) -> Result<Self> {
    let mut profile = Self::load().await?;
    let Some(path) = extra else {
      return Ok(profile);
    };
    let data = fs::read(path)
      .await
      .with_context(|| format!("read MCP configuration {}", path.display()))?;
    let file: ExternalMcpFile = serde_json::from_slice(&data)
      .with_context(|| format!("parse MCP configuration {}", path.display()))?;
    for (name, server) in file.servers {
      if server.command.trim().is_empty() {
        bail!("MCP server {name} has an empty command");
      }
      profile.servers.insert(
        name,
        McpServerConfig {
          transport: McpTransport::Stdio,
          command: std::iter::once(server.command).chain(server.args).collect(),
          url: None,
          header_env: BTreeMap::new(),
          headers: BTreeMap::new(),
          env: server.env,
          cwd: None,
          enabled: true,
          required: true,
          timeout_ms: default_timeout(),
        },
      );
    }
    profile.validate()?;
    Ok(profile)
  }

  pub fn validate(&self) -> Result<()> {
    for (name, server) in &self.servers {
      if !valid_name(name) {
        bail!("MCP server name {name:?} may only use letters, digits, '.', '_' and '-'");
      }
      if server.timeout_ms == 0 {
        bail!("MCP server {name} needs a timeout above zero");
      }
    }
    Ok(())
  }

  pub async fn save(&self) -> Result<()> {
    self.save_to(&Self::path()?).await
  }

  pub async fn save_to(&self, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).await?;
    }
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, toml::to_string_pretty(self)?).await?;
    // header and env values may be secrets, so the profile is private to the user
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).await?;
    }
    fs::rename(&temporary, path)
      .await
      .with_context(|| format!("write {}", path.display()))
  }
}

#[derive(Deserialize)]
struct ExternalMcpFile {
  #[serde(rename = "mcpServers")]
  servers: BTreeMap<String, ExternalMcpServer>,
}

#[derive(Deserialize)]
struct ExternalMcpServer {
  command: String,
  #[serde(default)]
  args: Vec<String>,
  #[serde(default)]
  env: BTreeMap<String, String>,
}

pub struct McpHub {
  servers: BTreeMap<String, Arc<Server>>,
}

impl McpHub {
  pub fn new(profile: McpProfile) -> Self {
    let servers = profile
      .servers
      .into_iter()
      .filter(|(_, config)| config.enabled)
      .map(|(name, config)| (name.clone(), Arc::new(Server::new(name, config))))
      .collect();
    Self { servers }
  }

  pub fn is_empty(&self) -> bool {
    self.servers.is_empty()
  }

  pub fn tool(self: Arc<Self>) -> Arc<dyn Tool> {
    Arc::new(McpTool::new(self))
  }

  pub fn server_names(&self) -> Vec<&str> {
    self.servers.keys().map(String::as_str).collect()
  }

  // required servers start together so startup pays for the slowest one, not the sum
  pub async fn ready(&self) -> Result<()> {
    let required = self
      .servers
      .values()
      .filter(|server| server.config.required);
    for result in join_all(required.map(|server| server.tools())).await {
      result?;
    }
    Ok(())
  }

  pub async fn instructions(&self) -> Result<Vec<(String, String)>> {
    let mut instructions = Vec::new();
    for (name, server) in &self.servers {
      if !server.config.required {
        continue;
      }
      if let Some(text) = server.instructions().await? {
        instructions.push((name.clone(), truncate(text, MAX_INSTRUCTIONS)));
      }
    }
    Ok(instructions)
  }

  pub(super) async fn tools(&self, server: &str) -> Result<Arc<[RemoteTool]>> {
    self.server(server)?.tools().await
  }

  // an optional server that already failed is left alone until something names it directly
  pub(super) async fn searchable_tools(&self) -> Result<Vec<(&str, Arc<[RemoteTool]>)>> {
    let attempts = self
      .servers
      .iter()
      .filter(|(_, server)| server.config.required || !server.failed())
      .map(|(name, server)| async move { (name.as_str(), server, server.tools().await) });
    let mut found = Vec::new();
    for (name, server, result) in join_all(attempts).await {
      match result {
        Ok(tools) => found.push((name, tools)),
        Err(error) if server.config.required => return Err(error),
        Err(_) => {}
      }
    }
    Ok(found)
  }

  pub(super) async fn call(&self, server: &str, name: &str, arguments: Value) -> Result<String> {
    self.server(server)?.call(name, arguments).await
  }

  // only what the server already told us counts; an unknown tool is treated as executing
  pub(super) fn cached_risk(&self, server: &str, name: &str) -> Risk {
    let Some(server) = self.servers.get(server) else {
      return Risk::Execute;
    };
    let ToolCache::Ready(tools) = &*server.cache() else {
      return Risk::Execute;
    };
    tools
      .iter()
      .find(|tool| tool.name == name)
      .map_or(Risk::Execute, |tool| tool.annotations.risk())
  }

  fn server(&self, name: &str) -> Result<&Arc<Server>> {
    self
      .servers
      .get(name)
      .with_context(|| format!("server {name} was not found"))
  }
}

struct Server {
  name: String,
  config: McpServerConfig,
  client: Mutex<Option<Client>>,
  tools: SyncMutex<ToolCache>,
}

enum ToolCache {
  Unknown,
  Ready(Arc<[RemoteTool]>),
  Failed,
}

impl Server {
  fn new(name: String, config: McpServerConfig) -> Self {
    Self {
      name,
      config,
      client: Mutex::new(None),
      tools: SyncMutex::new(ToolCache::Unknown),
    }
  }

  fn cache(&self) -> std::sync::MutexGuard<'_, ToolCache> {
    self.tools.lock().unwrap_or_else(PoisonError::into_inner)
  }

  fn failed(&self) -> bool {
    matches!(*self.cache(), ToolCache::Failed)
  }

  // a broken client is dropped, which ends its process, before a replacement starts
  async fn connect(&self) -> Result<MappedMutexGuard<'_, Client>> {
    let mut guard = self.client.lock().await;
    if guard.as_mut().is_none_or(Client::broken) {
      *guard = None;
      *guard = Some(Client::start(&self.name, &self.config).await?);
    }
    Ok(MutexGuard::map(guard, |client| {
      client.as_mut().expect("client was just started")
    }))
  }

  async fn tools(&self) -> Result<Arc<[RemoteTool]>> {
    if let ToolCache::Ready(tools) = &*self.cache() {
      return Ok(tools.clone());
    }
    let listed = async {
      let mut client = self.connect().await?;
      client.list_tools().await
    }
    .await;
    match listed {
      Ok(tools) => {
        let tools: Arc<[RemoteTool]> = tools.into();
        *self.cache() = ToolCache::Ready(tools.clone());
        Ok(tools)
      }
      Err(error) => {
        *self.cache() = ToolCache::Failed;
        Err(error)
      }
    }
  }

  async fn call(&self, name: &str, arguments: Value) -> Result<String> {
    let mut client = self.connect().await?;
    client.call_tool(name, arguments).await
  }

  async fn instructions(&self) -> Result<Option<String>> {
    let client = self.connect().await?;
    Ok(client.instructions().map(str::to_owned))
  }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct RemoteTool {
  pub name: String,
  #[serde(default)]
  pub description: String,
  #[serde(rename = "inputSchema")]
  pub input_schema: Value,
  #[serde(default)]
  pub annotations: ToolAnnotations,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ToolAnnotations {
  #[serde(default, rename = "readOnlyHint")]
  read_only: bool,
  #[serde(rename = "destructiveHint")]
  destructive: Option<bool>,
}

impl ToolAnnotations {
  fn risk(&self) -> Risk {
    if self.read_only {
      Risk::Read
    } else if self.destructive == Some(false) {
      Risk::Write
    } else {
      Risk::Execute
    }
  }
}
