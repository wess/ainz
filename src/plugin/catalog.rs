use std::{
  collections::{BTreeMap, BTreeSet},
  path::{Component, Path, PathBuf},
  sync::Arc,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::fs;

use super::{
  PluginFormat, PluginManifest, PluginMeta, PluginRuntime, RuntimeKind,
  component::{ComponentRuntime, ComponentTool},
  process::ProcessTool,
};
use crate::mcp::{McpProfile, McpServerConfig, McpTransport};
use crate::tool::Tool;

#[derive(Clone, Debug)]
pub struct DiscoveredPlugin {
  pub manifest: PluginManifest,
  pub path: PathBuf,
  pub fingerprint: String,
  pub approved: bool,
  pub format: PluginFormat,
}

#[derive(Clone, Debug, Default)]
pub struct PluginCatalog {
  pub plugins: Vec<DiscoveredPlugin>,
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
    let mut plugins = BTreeMap::new();
    for root in roots {
      for plugin in discover_root(&root, &grants).await? {
        plugins.insert(plugin.manifest.plugin.name.clone(), plugin);
      }
    }
    Ok(Self {
      plugins: plugins.into_values().collect(),
    })
  }

  pub fn approved_tools(&self) -> Result<Vec<Arc<dyn Tool>>> {
    let mut tools = Vec::new();
    for plugin in self
      .plugins
      .iter()
      .filter(|plugin| plugin.approved && plugin.manifest.plugin.enabled)
    {
      let root = plugin.path.parent().unwrap_or(Path::new(".")).to_path_buf();
      match plugin.manifest.runtime.kind {
        RuntimeKind::Process => {
          for definition in &plugin.manifest.tools {
            tools.push(Arc::new(ProcessTool::new(
              plugin.manifest.clone(),
              root.clone(),
              definition.clone(),
            )) as Arc<dyn Tool>);
          }
        }
        RuntimeKind::Component => {
          let runtime = Arc::new(ComponentRuntime::new(&plugin.manifest, &root)?);
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
      .map(|plugin| {
        plugin
          .path
          .parent()
          .unwrap_or(Path::new("."))
          .join("skills")
      })
      .collect()
  }

  pub async fn merge_mcp(&self, mut profile: McpProfile) -> Result<McpProfile> {
    for plugin in self
      .plugins
      .iter()
      .filter(|plugin| plugin.approved && plugin.format == PluginFormat::AgentPlugin)
    {
      if merge_plugin_mcp(&mut profile, plugin).await.is_err() {
        continue;
      }
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

async fn discover_root(root: &Path, grants: &PluginGrants) -> Result<Vec<DiscoveredPlugin>> {
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
    let native_path = directory.join("plugin.toml");
    if let Ok(data) = fs::read(&native_path).await {
      let manifest: PluginManifest =
        toml::from_slice(&data).with_context(|| format!("parse {}", native_path.display()))?;
      validate(&manifest).with_context(|| format!("validate {}", native_path.display()))?;
      let fingerprint = fingerprint(&data, &manifest, &native_path).await?;
      let approved = grants.grants.get(&manifest.plugin.name) == Some(&fingerprint);
      plugins.push(DiscoveredPlugin {
        manifest,
        path: native_path,
        fingerprint,
        approved,
        format: PluginFormat::AgentX,
      });
      continue;
    }

    let portable_path = directory.join("plugin.json");
    let data = match fs::read(&portable_path).await {
      Ok(data) => data,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => {
        return Err(error).with_context(|| format!("read {}", portable_path.display()));
      }
    };
    let portable: AgentPluginManifest = serde_json::from_slice(&data)
      .with_context(|| format!("parse {}", portable_path.display()))?;
    validate_portable(&portable)
      .with_context(|| format!("validate {}", portable_path.display()))?;
    let manifest = PluginManifest {
      plugin: PluginMeta {
        name: portable.name,
        version: portable.version,
        api: 1,
        enabled: true,
      },
      runtime: PluginRuntime {
        kind: RuntimeKind::Process,
        command: Vec::new(),
        path: None,
        timeout_ms: 30_000,
        memory_bytes: 64 * 1024 * 1024,
        fuel: 10_000_000,
      },
      capabilities: Vec::new(),
      tools: Vec::new(),
    };
    let fingerprint = portable_fingerprint(&directory, &data).await?;
    let approved = grants.grants.get(&manifest.plugin.name) == Some(&fingerprint);
    plugins.push(DiscoveredPlugin {
      manifest,
      path: portable_path,
      fingerprint,
      approved,
      format: PluginFormat::AgentPlugin,
    });
  }
  Ok(plugins)
}

fn validate(manifest: &PluginManifest) -> Result<()> {
  valid_name(&manifest.plugin.name)?;
  if manifest.plugin.api != 1 {
    bail!("unsupported plugin API {}; expected 1", manifest.plugin.api);
  }
  match manifest.runtime.kind {
    RuntimeKind::Process if manifest.runtime.command.is_empty() => {
      bail!("process runtime.command cannot be empty");
    }
    RuntimeKind::Component if manifest.runtime.path.is_none() => {
      bail!("component runtime.path is required");
    }
    _ => {}
  }
  if manifest.runtime.memory_bytes == 0 || manifest.runtime.fuel == 0 {
    bail!("runtime memory_bytes and fuel must be greater than zero");
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
    let parameters = serde_json::to_value(&tool.parameters)?;
    if parameters.get("type").and_then(serde_json::Value::as_str) != Some("object") {
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

async fn fingerprint(
  data: &[u8],
  manifest: &PluginManifest,
  manifest_path: &Path,
) -> Result<String> {
  let root = manifest_path.parent().unwrap_or(Path::new("."));
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
  let bytes = fs::read(&artifact)
    .await
    .with_context(|| format!("read runtime artifact {}", artifact.display()))?;
  let mut hash = Sha256::new();
  hash.update(data);
  hash.update([0]);
  hash.update(bytes);
  Ok(format!("{:x}", hash.finalize()))
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
#[serde(deny_unknown_fields)]
struct AgentMcpFile {
  #[serde(rename = "$schema")]
  schema: String,
  #[serde(rename = "mcpServers")]
  servers: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
  if name.is_empty()
    || name.len() > 64
    || !name
      .chars()
      .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '.')
    || !name
      .chars()
      .next()
      .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    || !name
      .chars()
      .last()
      .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
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

async fn portable_fingerprint(root: &Path, _manifest: &[u8]) -> Result<String> {
  let mut pending = vec![root.to_path_buf()];
  let mut files = Vec::new();
  while let Some(directory) = pending.pop() {
    let mut entries = fs::read_dir(&directory)
      .await
      .with_context(|| format!("read {}", directory.display()))?;
    while let Some(entry) = entries.next_entry().await? {
      let file_type = entry.file_type().await?;
      if file_type.is_symlink() {
        continue;
      }
      if file_type.is_dir() {
        pending.push(entry.path());
      } else if file_type.is_file() {
        let path = entry.path();
        let relative = path
          .strip_prefix(root)
          .with_context(|| format!("resolve {}", path.display()))?
          .to_string_lossy()
          .into_owned();
        files.push((relative, fs::read(&path).await?));
      }
    }
  }
  files.sort_by(|left, right| left.0.cmp(&right.0));
  let mut hash = Sha256::new();
  for (path, data) in files {
    hash.update(path.as_bytes());
    hash.update([0]);
    hash.update(data);
    hash.update([0]);
  }
  Ok(format!("{:x}", hash.finalize()))
}

async fn merge_plugin_mcp(profile: &mut McpProfile, plugin: &DiscoveredPlugin) -> Result<()> {
  let root = plugin.path.parent().unwrap_or(Path::new("."));
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
  for (server_name, value) in file.servers {
    let Ok(server) = serde_json::from_value::<AgentMcpServer>(value) else {
      continue;
    };
    let name = format!("{}__{server_name}", plugin.manifest.plugin.name);
    let config = match server.kind.as_str() {
      "stdio" => {
        let Some(command) = server.command else {
          continue;
        };
        let command = if let Some(relative) = command.strip_prefix("./") {
          let relative = Path::new(relative);
          if relative
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
          {
            continue;
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
          continue;
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
      _ => continue,
    };
    profile.servers.entry(name).or_insert(config);
  }
  Ok(())
}
