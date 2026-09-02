use std::{
  collections::{BTreeMap, BTreeSet},
  path::{Component, Path, PathBuf},
  sync::Arc,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncReadExt};

use super::{
  MAX_MEMORY_BYTES, MAX_TIMEOUT_MS, PluginFormat, PluginManifest, PluginMeta, PluginRuntime,
  RuntimeKind,
  component::{ComponentRuntime, ComponentTool},
  process::ProcessTool,
};
use crate::mcp::{McpProfile, McpServerConfig, McpTransport};
use crate::tool::Tool;

const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DiscoveredPlugin {
  pub manifest: Arc<PluginManifest>,
  pub path: PathBuf,
  // pins the manifest plus every file under the plugin directory
  pub fingerprint: String,
  // pins the executable or component on its own so it can be rechecked at run time
  pub artifact_digest: String,
  pub approved: bool,
  pub format: PluginFormat,
}

impl DiscoveredPlugin {
  pub fn root(&self) -> &Path {
    self.path.parent().unwrap_or(Path::new("."))
  }

  // the artifact as the manifest names it, for listings and approval prompts
  pub fn artifact(&self) -> Option<PathBuf> {
    match self.manifest.runtime.kind {
      RuntimeKind::Process => self.manifest.runtime.command.first().map(PathBuf::from),
      RuntimeKind::Component => self.manifest.runtime.path.clone(),
    }
  }
}

#[derive(Clone, Debug, Default)]
pub struct PluginCatalog {
  pub plugins: Vec<DiscoveredPlugin>,
  // plugin directories that could not be read or validated; they never block the others
  pub issues: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PluginGrants {
  #[serde(default)]
  grants: BTreeMap<String, String>,
}

impl PluginCatalog {
  pub async fn discover(workspace: &Path) -> Result<Self> {
    let path = PluginGrants::path()?;
    if !path.exists() {
      let legacy = dirs::data_local_dir()
        .context("could not locate the data directory")?
        .join("struts/plugins.json");
      if legacy.exists() {
        let grants = PluginGrants::load(&legacy).await?;
        grants.save(&path).await?;
      }
    }
    Self::discover_with_grants(workspace, &path).await
  }

  pub async fn discover_with_grants(workspace: &Path, grants_path: &Path) -> Result<Self> {
    let grants = PluginGrants::load(grants_path).await?;
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
      roots.push(config.join("struts/plugins"));
      roots.push(config.join("agentx/plugins"));
    }
    if let Some(home) = dirs::home_dir() {
      roots.push(home.join(".agents/plugins"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    for path in ancestors {
      roots.push(path.join(".agents/plugins"));
      roots.push(path.join(".struts/plugins"));
      roots.push(path.join(".agentx/plugins"));
    }
    roots.dedup();
    let mut catalog = Self::default();
    let mut plugins = BTreeMap::new();
    for root in roots {
      for plugin in discover_root(&root, &grants, &mut catalog.issues).await? {
        plugins.insert(plugin.manifest.plugin.name.clone(), plugin);
      }
    }
    catalog.plugins = plugins.into_values().collect();
    Ok(catalog)
  }

  pub async fn approved_tools(&self) -> Result<Vec<Arc<dyn Tool>>> {
    let mut tools = Vec::new();
    for plugin in self
      .plugins
      .iter()
      .filter(|plugin| plugin.approved && plugin.manifest.plugin.enabled)
    {
      match plugin.manifest.runtime.kind {
        RuntimeKind::Process => {
          for definition in &plugin.manifest.tools {
            tools.push(Arc::new(ProcessTool::new(
              plugin.manifest.clone(),
              plugin.root(),
              plugin.artifact_digest.clone(),
              definition.clone(),
            )?) as Arc<dyn Tool>);
          }
        }
        RuntimeKind::Component => {
          let runtime = Arc::new(
            ComponentRuntime::new(&plugin.manifest, plugin.root(), &plugin.artifact_digest).await?,
          );
          for definition in &plugin.manifest.tools {
            tools.push(Arc::new(ComponentTool::new(
              runtime.clone(),
              plugin.manifest.plugin.name.clone(),
              definition.clone(),
            )) as Arc<dyn Tool>);
          }
        }
      }
    }
    Ok(tools)
  }

  pub fn approved_skill_roots(&self) -> Vec<PathBuf> {
    self
      .plugins
      .iter()
      .filter(|plugin| plugin.approved && plugin.format == PluginFormat::AgentPlugin)
      .map(|plugin| plugin.root().join("skills"))
      .collect()
  }

  pub async fn merge_mcp(&self, mut profile: McpProfile) -> Result<McpProfile> {
    for plugin in self
      .plugins
      .iter()
      .filter(|plugin| plugin.approved && plugin.format == PluginFormat::AgentPlugin)
    {
      merge_plugin_mcp(&mut profile, plugin)
        .await
        .with_context(|| format!("plugin {}", plugin.manifest.plugin.name))?;
    }
    Ok(profile)
  }

  pub async fn approve(&mut self, name: &str) -> Result<()> {
    self.approve_with(name, &PluginGrants::path()?).await
  }

  pub async fn approve_with(&mut self, name: &str, grants_path: &Path) -> Result<()> {
    let plugin = self
      .plugins
      .iter_mut()
      .find(|plugin| plugin.manifest.plugin.name == name)
      .with_context(|| format!("plugin {name} was not found"))?;
    let mut grants = PluginGrants::load(grants_path).await?;
    grants
      .grants
      .insert(name.into(), plugin.fingerprint.clone());
    grants.save(grants_path).await?;
    plugin.approved = true;
    Ok(())
  }

  pub async fn revoke(&mut self, name: &str) -> Result<()> {
    self.revoke_with(name, &PluginGrants::path()?).await
  }

  pub async fn revoke_with(&mut self, name: &str, grants_path: &Path) -> Result<()> {
    let mut grants = PluginGrants::load(grants_path).await?;
    grants.grants.remove(name);
    grants.save(grants_path).await?;
    if let Some(plugin) = self
      .plugins
      .iter_mut()
      .find(|plugin| plugin.manifest.plugin.name == name)
    {
      plugin.approved = false;
    }
    Ok(())
  }
}

impl PluginGrants {
  fn path() -> Result<PathBuf> {
    Ok(
      dirs::data_local_dir()
        .context("could not locate the data directory")?
        .join("agentx/plugins.json"),
    )
  }

  async fn load(path: &Path) -> Result<Self> {
    match fs::read(path).await {
      Ok(data) => {
        serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))
      }
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
      Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
  }

  async fn save(&self, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).await?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(self)?).await?;
    fs::rename(temp, path).await?;
    Ok(())
  }
}

async fn discover_root(
  root: &Path,
  grants: &PluginGrants,
  issues: &mut Vec<String>,
) -> Result<Vec<DiscoveredPlugin>> {
  let mut entries = match fs::read_dir(root).await {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(error).with_context(|| format!("read {}", root.display())),
  };
  let mut plugins = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    if !entry.file_type().await?.is_dir() {
      continue;
    }
    let directory = entry.path();
    match discover_plugin(&directory, grants).await {
      Ok(Some(plugin)) => plugins.push(plugin),
      Ok(None) => {}
      Err(error) => issues.push(format!("{}: {error:#}", directory.display())),
    }
  }
  Ok(plugins)
}

async fn discover_plugin(
  directory: &Path,
  grants: &PluginGrants,
) -> Result<Option<DiscoveredPlugin>> {
  let native_path = directory.join("plugin.toml");
  if let Ok(data) = fs::read(&native_path).await {
    let manifest: PluginManifest =
      toml::from_slice(&data).with_context(|| format!("parse {}", native_path.display()))?;
    validate(&manifest).with_context(|| format!("validate {}", native_path.display()))?;
    let (fingerprint, artifact_digest) = fingerprint(&data, &manifest, directory).await?;
    let approved = grants.grants.get(&manifest.plugin.name) == Some(&fingerprint);
    return Ok(Some(DiscoveredPlugin {
      manifest: Arc::new(manifest),
      path: native_path,
      fingerprint,
      artifact_digest,
      approved,
      format: PluginFormat::AgentX,
    }));
  }

  let portable_path = directory.join("plugin.json");
  let data = match fs::read(&portable_path).await {
    Ok(data) => data,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(error) => {
      return Err(error).with_context(|| format!("read {}", portable_path.display()));
    }
  };
  let portable: AgentPluginManifest =
    serde_json::from_slice(&data).with_context(|| format!("parse {}", portable_path.display()))?;
  validate_portable(&portable).with_context(|| format!("validate {}", portable_path.display()))?;
  let mut hash = Sha256::new();
  tree_digest(directory, &mut hash).await?;
  let fingerprint = format!("{:x}", hash.finalize());
  let approved = grants.grants.get(&portable.name) == Some(&fingerprint);
  Ok(Some(DiscoveredPlugin {
    manifest: Arc::new(portable_manifest(portable.name, portable.version)),
    path: portable_path,
    fingerprint,
    artifact_digest: String::new(),
    approved,
    format: PluginFormat::AgentPlugin,
  }))
}

// portable packages carry skills and MCP servers, never tools, so the manifest is a stub
fn portable_manifest(name: String, version: String) -> PluginManifest {
  PluginManifest {
    plugin: PluginMeta {
      name,
      version,
      api: 1,
      enabled: true,
    },
    runtime: PluginRuntime {
      kind: RuntimeKind::Process,
      command: Vec::new(),
      path: None,
      timeout_ms: 30_000,
      memory_bytes: 0,
      fuel: 0,
    },
    capabilities: Vec::new(),
    tools: Vec::new(),
  }
}

fn validate(manifest: &PluginManifest) -> Result<()> {
  valid_name(&manifest.plugin.name)?;
  if manifest.plugin.api != 1 {
    bail!("unsupported plugin API {}; expected 1", manifest.plugin.api);
  }
  let runtime = &manifest.runtime;
  match runtime.kind {
    RuntimeKind::Process if runtime.command.is_empty() => {
      bail!("process runtime.command cannot be empty");
    }
    RuntimeKind::Component if runtime.path.is_none() => {
      bail!("component runtime.path is required");
    }
    RuntimeKind::Component if runtime.memory_bytes == 0 || runtime.fuel == 0 => {
      bail!("runtime memory_bytes and fuel must be greater than zero");
    }
    RuntimeKind::Component if runtime.memory_bytes > MAX_MEMORY_BYTES => {
      bail!("runtime memory_bytes may not exceed {MAX_MEMORY_BYTES}");
    }
    _ => {}
  }
  if runtime.timeout_ms == 0 || runtime.timeout_ms > MAX_TIMEOUT_MS {
    bail!("runtime timeout_ms must be between 1 and {MAX_TIMEOUT_MS}");
  }
  if manifest.tools.is_empty() {
    bail!("a plugin must expose at least one tool");
  }
  let mut names = BTreeSet::new();
  for tool in &manifest.tools {
    valid_name(&tool.name)?;
    if !names.insert(&tool.name) {
      bail!("duplicate tool name: {}", tool.name);
    }
    if tool.description.trim().is_empty() {
      bail!("tool {} must have a description", tool.name);
    }
    if tool.parameters.get("type").and_then(Value::as_str) != Some("object") {
      bail!("tool {} parameters must be an object schema", tool.name);
    }
    if tool.capabilities.is_empty() {
      bail!("tool {} must declare at least one capability", tool.name);
    }
    for capability in &tool.capabilities {
      if !manifest.capabilities.contains(capability) {
        bail!(
          "tool {} uses undeclared capability {capability:?}",
          tool.name
        );
      }
    }
  }
  Ok(())
}

// the manifest bytes, every file under the plugin directory, then the artifact, which may
// live outside the directory. the artifact digest is returned alone for run-time rechecks
async fn fingerprint(
  data: &[u8],
  manifest: &PluginManifest,
  root: &Path,
) -> Result<(String, String)> {
  let artifact = match manifest.runtime.kind {
    RuntimeKind::Process => manifest.runtime.command.first().map(PathBuf::from),
    RuntimeKind::Component => manifest.runtime.path.clone(),
  }
  .context("runtime artifact is missing")?;
  let artifact = if artifact.is_relative() {
    root.join(artifact)
  } else {
    artifact
  };
  let artifact_digest = file_digest(&artifact).await?;
  let mut hash = Sha256::new();
  hash.update(data);
  hash.update([0]);
  tree_digest(root, &mut hash).await?;
  hash.update(artifact_digest.as_bytes());
  Ok((format!("{:x}", hash.finalize()), artifact_digest))
}

pub(super) fn digest(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}

// only regular files of a sane size count as artifacts; a FIFO or device must not stall discovery
pub(super) async fn read_artifact(path: &Path) -> Result<Vec<u8>> {
  let metadata = fs::metadata(path)
    .await
    .with_context(|| format!("read runtime artifact {}", path.display()))?;
  if !metadata.is_file() {
    bail!("runtime artifact {} is not a regular file", path.display());
  }
  if metadata.len() > MAX_ARTIFACT_BYTES {
    bail!(
      "runtime artifact {} exceeds {MAX_ARTIFACT_BYTES} bytes",
      path.display()
    );
  }
  fs::read(path)
    .await
    .with_context(|| format!("read runtime artifact {}", path.display()))
}

pub(super) async fn file_digest(path: &Path) -> Result<String> {
  Ok(digest(&read_artifact(path).await?))
}

// streams every regular file under root into the hash in path order; symlinks contribute
// their target so a re-pointed link changes the pin too
async fn tree_digest(root: &Path, hash: &mut Sha256) -> Result<()> {
  let mut pending = vec![root.to_path_buf()];
  let mut files = Vec::new();
  while let Some(directory) = pending.pop() {
    let mut entries = fs::read_dir(&directory)
      .await
      .with_context(|| format!("read {}", directory.display()))?;
    while let Some(entry) = entries.next_entry().await? {
      let file_type = entry.file_type().await?;
      if file_type.is_dir() {
        pending.push(entry.path());
      } else if file_type.is_file() || file_type.is_symlink() {
        files.push(entry.path());
      }
    }
  }
  files.sort();
  let mut buffer = vec![0_u8; 64 * 1024];
  for path in files {
    let relative = path
      .strip_prefix(root)
      .with_context(|| format!("resolve {}", path.display()))?;
    hash.update(relative.to_string_lossy().as_bytes());
    hash.update([0]);
    let metadata = fs::symlink_metadata(&path).await?;
    if metadata.is_symlink() {
      hash.update(b"link:");
      hash.update(fs::read_link(&path).await?.to_string_lossy().as_bytes());
    } else {
      let mut file = fs::File::open(&path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
      loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
          break;
        }
        hash.update(&buffer[..read]);
      }
    }
    hash.update([0]);
  }
  Ok(())
}

fn valid_name(name: &str) -> Result<()> {
  if name.is_empty()
    || !name.chars().all(|character| {
      character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    })
  {
    bail!("name must contain only lowercase letters, numbers, and underscores");
  }
  Ok(())
}

const AGENT_PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const AGENT_MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

#[derive(Deserialize)]
struct AgentPluginManifest {
  #[serde(rename = "$schema")]
  schema: String,
  name: String,
  #[serde(default = "portable_version")]
  version: String,
  #[serde(default)]
  extensions: Value,
}

fn portable_version() -> String {
  "0.0.0".into()
}

#[derive(Deserialize)]
struct AgentMcpFile {
  #[serde(rename = "$schema")]
  schema: String,
  #[serde(rename = "mcpServers")]
  servers: BTreeMap<String, AgentMcpServer>,
}

#[derive(Deserialize)]
struct AgentMcpServer {
  #[serde(rename = "type")]
  kind: String,
  command: Option<String>,
  #[serde(default)]
  args: Vec<String>,
  #[serde(default)]
  env: BTreeMap<String, String>,
  cwd: Option<String>,
  url: Option<String>,
  #[serde(default)]
  headers: BTreeMap<String, String>,
}

fn validate_portable(manifest: &AgentPluginManifest) -> Result<()> {
  if manifest.schema != AGENT_PLUGIN_SCHEMA {
    bail!("unsupported Agent Plugins schema {}", manifest.schema);
  }
  let name = &manifest.name;
  let plain = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
  if name.is_empty()
    || name.len() > 64
    || !name.chars().all(|ch| plain(ch) || ch == '-' || ch == '.')
    || !name.chars().next().is_some_and(plain)
    || !name.chars().last().is_some_and(plain)
    || name.contains("--")
    || name.contains("..")
  {
    bail!("invalid Agent Plugins name {name}");
  }
  if !manifest.extensions.is_null() && !manifest.extensions.is_object() {
    bail!("extensions must be an object");
  }
  Ok(())
}

async fn merge_plugin_mcp(profile: &mut McpProfile, plugin: &DiscoveredPlugin) -> Result<()> {
  let root = plugin.root();
  let path = root.join("mcp.json");
  let data = match fs::read(&path).await {
    Ok(data) => data,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
    Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
  };
  let file: AgentMcpFile =
    serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))?;
  if file.schema != AGENT_MCP_SCHEMA {
    bail!("unsupported Agent Plugins MCP schema {}", file.schema);
  }
  let data_root = dirs::data_local_dir()
    .context("could not locate the data directory")?
    .join("agentx/plugin-data")
    .join(&plugin.manifest.plugin.name);
  fs::create_dir_all(&data_root).await?;
  let root_text = root.to_string_lossy();
  let data_text = data_root.to_string_lossy();
  for (server_name, server) in file.servers {
    let name = format!("{}__{server_name}", plugin.manifest.plugin.name);
    let config = match server.kind.as_str() {
      "stdio" => {
        let Some(command) = server.command else {
          bail!("MCP server {server_name} has no command");
        };
        // a ./ command stays inside the package; anything else is looked up like a normal command
        let command = if let Some(relative) = command.strip_prefix("./") {
          let relative = Path::new(relative);
          if relative
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
          {
            bail!("MCP server {server_name} command escapes the plugin directory");
          }
          root.join(relative).to_string_lossy().into_owned()
        } else {
          command
        };
        let expand = |value: String| {
          value
            .replace("${PLUGIN_ROOT}", &root_text)
            .replace("${PLUGIN_DATA}", &data_text)
        };
        let mut env: BTreeMap<_, _> = server
          .env
          .into_iter()
          .map(|(name, value)| (name, expand(value)))
          .collect();
        env.insert("PLUGIN_ROOT".into(), root_text.to_string());
        env.insert("PLUGIN_DATA".into(), data_text.to_string());
        let cwd = server
          .cwd
          .map(&expand)
          .map(PathBuf::from)
          .unwrap_or_else(|| root.to_path_buf());
        if cwd.components().any(|part| part == Component::ParentDir)
          || !(cwd.starts_with(root) || cwd.starts_with(&data_root))
        {
          bail!("MCP server {server_name} cwd must stay inside the plugin or its data directory");
        }
        McpServerConfig {
          transport: McpTransport::Stdio,
          command: std::iter::once(command)
            .chain(server.args.into_iter().map(expand))
            .collect(),
          url: None,
          header_env: BTreeMap::new(),
          headers: BTreeMap::new(),
          env,
          cwd: Some(cwd),
          enabled: true,
          required: false,
          timeout_ms: 30_000,
        }
      }
      "streamable-http" => McpServerConfig {
        transport: McpTransport::StreamableHttp,
        command: Vec::new(),
        url: server.url,
        header_env: BTreeMap::new(),
        headers: server.headers,
        env: BTreeMap::new(),
        cwd: None,
        enabled: true,
        required: false,
        timeout_ms: 30_000,
      },
      other => bail!("MCP server {server_name} has unsupported type {other}"),
    };
    profile.servers.entry(name).or_insert(config);
  }
  Ok(())
}
