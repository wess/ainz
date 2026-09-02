mod client;
mod tool;

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::Arc,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{fs, sync::Mutex};

use crate::tool::Tool;

use self::{client::Client, tool::McpTool};

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
  #[serde(default)]
  command: CommandField,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(untagged)]
enum CommandField {
  String(String),
  List(Vec<String>),
  #[default]
  Empty,
}

impl From<McpServerConfigFile> for McpServerConfig {
  fn from(file: McpServerConfigFile) -> Self {
    let command = match file.command {
      CommandField::String(command) => std::iter::once(command).chain(file.args).collect(),
      CommandField::List(command) => command.into_iter().chain(file.args).collect(),
      CommandField::Empty => file.args,
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
      command: command.next().map(CommandField::String).unwrap_or_default(),
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

impl McpProfile {
  pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AGENTX_MCP_PROFILE") {
      return Ok(PathBuf::from(path));
    }
    Ok(
      dirs::config_dir()
        .context("could not locate the config directory")?
        .join("agentx/mcp.toml"),
    )
  }

  pub async fn load() -> Result<Self> {
    let path = Self::path()?;
    let legacy = dirs::config_dir()
      .context("could not locate the config directory")?
      .join("struts/mcp.toml");
    let source =
      if !path.exists() && std::env::var_os("AGENTX_MCP_PROFILE").is_none() && legacy.exists() {
        &legacy
      } else {
        &path
      };
    let profile = match fs::read_to_string(source).await {
      Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display())),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
      Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }?;
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
        anyhow::bail!("MCP server {name} has an empty command");
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
    Ok(profile)
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

  pub async fn load() -> Result<Self> {
    Ok(Self::new(McpProfile::load().await?))
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

  pub async fn ready(&self) -> Result<()> {
    for server in self
      .servers
      .values()
      .filter(|server| server.config.required)
    {
      server.tools().await?;
    }
    Ok(())
  }

  pub async fn instructions(&self) -> Result<Vec<String>> {
    let mut instructions = Vec::new();
    for server in self
      .servers
      .values()
      .filter(|server| server.config.required)
    {
      if let Some(value) = server.instructions().await? {
        instructions.push(value);
      }
    }
    Ok(instructions)
  }

  pub(super) async fn tools(&self, server: &str) -> Result<Vec<RemoteTool>> {
    self
      .servers
      .get(server)
      .with_context(|| format!("server {server} was not found"))?
      .tools()
      .await
  }

  pub(super) async fn searchable_tools(&self) -> Result<Vec<(&str, Vec<RemoteTool>)>> {
    let mut found = Vec::new();
    for (name, server) in &self.servers {
      match server.tools().await {
        Ok(tools) => found.push((name.as_str(), tools)),
        Err(error) if server.config.required => return Err(error),
        Err(_) => {}
      }
    }
    Ok(found)
  }

  pub(super) async fn call(&self, server: &str, name: &str, arguments: Value) -> Result<String> {
    self
      .servers
      .get(server)
      .with_context(|| format!("server {server} was not found"))?
      .call(name, arguments)
      .await
  }
}

struct Server {
  name: String,
  config: McpServerConfig,
  client: Mutex<Option<Client>>,
  tools: Mutex<Option<Vec<RemoteTool>>>,
}

impl Server {
  fn new(name: String, config: McpServerConfig) -> Self {
    Self {
      name,
      config,
      client: Mutex::new(None),
      tools: Mutex::new(None),
    }
  }

  async fn tools(&self) -> Result<Vec<RemoteTool>> {
    if let Some(tools) = self.tools.lock().await.clone() {
      return Ok(tools);
    }
    let mut client = self.client.lock().await;
    if client.is_none() {
      *client = Some(Client::start(&self.name, &self.config).await?);
    }
    let tools = client.as_mut().unwrap().list_tools().await?;
    *self.tools.lock().await = Some(tools.clone());
    Ok(tools)
  }

  async fn call(&self, name: &str, arguments: Value) -> Result<String> {
    let mut client = self.client.lock().await;
    if client.is_none() {
      *client = Some(Client::start(&self.name, &self.config).await?);
    }
    client.as_mut().unwrap().call_tool(name, arguments).await
  }

  async fn instructions(&self) -> Result<Option<String>> {
    let mut client = self.client.lock().await;
    if client.is_none() {
      *client = Some(Client::start(&self.name, &self.config).await?);
    }
    Ok(
      client
        .as_ref()
        .and_then(Client::instructions)
        .map(str::to_owned),
    )
  }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct RemoteTool {
  pub name: String,
  #[serde(default)]
  pub description: String,
  #[serde(rename = "inputSchema")]
  pub input_schema: Value,
}
