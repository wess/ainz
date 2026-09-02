use std::{
  collections::BTreeMap,
  env,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
  #[default]
  Ask,
  Auto,
  ReadOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
  Http,
  Process,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessOutput {
  #[default]
  Text,
  JsonResult,
  /// One JSON object per line, read as the command writes it, so a long run is visible while
  /// it happens instead of arriving all at once at the end.
  StreamJson,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBackend {
  Off,
  #[default]
  Local,
  Synapse,
}

impl MemoryBackend {
  pub fn label(self) -> &'static str {
    match self {
      Self::Off => "off",
      Self::Local => "local",
      Self::Synapse => "synapse",
    }
  }

  pub fn parse(value: &str) -> Result<Self> {
    match value.trim() {
      "off" | "none" => Ok(Self::Off),
      "local" => Ok(Self::Local),
      "synapse" => Ok(Self::Synapse),
      other => bail!("unknown memory backend {other}; use off, local, or synapse"),
    }
  }
}

// Synapse is optional: nothing here starts unless the user turns it on and the binary exists
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SynapseConfig {
  pub enabled: bool,
  pub mesh: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub command: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct MemoryConfig {
  pub backend: MemoryBackend,
  pub recall_on_start: bool,
  pub recall_limit: usize,
  pub remember_on_compact: bool,
  pub teach: bool,
}

impl Default for MemoryConfig {
  fn default() -> Self {
    Self {
      backend: MemoryBackend::Local,
      recall_on_start: true,
      recall_limit: 5,
      remember_on_compact: true,
      teach: false,
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct UiConfig {
  pub roster_visible: bool,
  pub header: String,
  pub vim: bool,
  /// Draw in the terminal's own scroll instead of taking the whole screen, so finished
  /// transcript stays in the scrollback the terminal already keeps.
  pub inline: bool,
}

impl Default for UiConfig {
  fn default() -> Self {
    Self {
      roster_visible: true,
      header: "random".into(),
      vim: false,
      inline: false,
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderConfig {
  pub kind: ProviderKind,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub endpoint: Option<String>,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub api_key_env: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub command: Option<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub args: Vec<String>,
  #[serde(default, skip_serializing_if = "is_text_output")]
  pub output: ProcessOutput,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub models: Vec<String>,
}

fn default_api_key_env() -> String {
  "AINZ_API_KEY".into()
}

fn is_text_output(output: &ProcessOutput) -> bool {
  *output == ProcessOutput::Text
}

fn strings(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_string()).collect()
}

impl ProviderConfig {
  pub fn http(endpoint: impl Into<String>, api_key_env: impl Into<String>) -> Self {
    Self {
      kind: ProviderKind::Http,
      endpoint: Some(endpoint.into()),
      api_key_env: api_key_env.into(),
      command: None,
      args: Vec::new(),
      output: ProcessOutput::Text,
      models: Vec::new(),
    }
  }

  pub fn process(command: impl Into<String>, args: Vec<String>, output: ProcessOutput) -> Self {
    Self {
      kind: ProviderKind::Process,
      endpoint: None,
      api_key_env: String::new(),
      command: Some(command.into()),
      args,
      output,
      models: Vec::new(),
    }
  }

  pub fn ollama() -> Self {
    Self::http("http://127.0.0.1:11434/v1", "")
  }

  /// A LiteLLM proxy speaks the same chat-completions API for every model behind it.
  pub fn lite_llm() -> Self {
    Self::http("http://127.0.0.1:4000/v1", "LITELLM_API_KEY")
  }

  pub fn codex() -> Self {
    Self::process(
      "codex",
      strings(&[
        "exec",
        "--ephemeral",
        "--color",
        "never",
        "--sandbox",
        "{sandbox}",
        "-C",
        "{workspace}",
        "--model",
        "{model}",
        "-",
      ]),
      ProcessOutput::Text,
    )
  }

  pub fn claude_code() -> Self {
    Self::process(
      "claude",
      strings(&[
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--no-session-persistence",
        "--model",
        "{model}",
        "--permission-mode",
        "{permission}",
      ]),
      ProcessOutput::StreamJson,
    )
  }

  pub fn validate(&self, name: &str) -> Result<()> {
    match self.kind {
      ProviderKind::Http if self.endpoint.as_deref().is_none_or(str::is_empty) => {
        bail!("HTTP provider {name} requires an endpoint")
      }
      ProviderKind::Process if self.command.as_deref().is_none_or(str::is_empty) => {
        bail!("process provider {name} requires a command")
      }
      _ => Ok(()),
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
  pub provider: Option<String>,
  pub endpoint: String,
  pub model: String,
  pub api_key_env: String,
  pub providers: BTreeMap<String, ProviderConfig>,
  pub max_steps: usize,
  pub max_output_bytes: usize,
  pub context_tokens: usize,
  pub compact_at_tokens: usize,
  pub preserve_messages: usize,
  pub permissions: PermissionMode,
  pub ui: UiConfig,
  pub memory: MemoryConfig,
  pub synapse: SynapseConfig,
  #[serde(skip)]
  pub mcp_config: Option<PathBuf>,
  // set for one session by --yeet or /yeet; never written to the file, because running wide
  // open is a decision about this run rather than a preference
  #[serde(skip)]
  pub yeet: bool,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      provider: None,
      endpoint: "http://127.0.0.1:11434/v1".into(),
      model: "".into(),
      api_key_env: default_api_key_env(),
      providers: BTreeMap::new(),
      max_steps: 32,
      max_output_bytes: 64 * 1024,
      context_tokens: 128_000,
      compact_at_tokens: 96_000,
      preserve_messages: 8,
      permissions: PermissionMode::Ask,
      ui: UiConfig::default(),
      memory: MemoryConfig::default(),
      synapse: SynapseConfig::default(),
      mcp_config: None,
      yeet: false,
    }
  }
}

impl Config {
  pub fn path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("AINZ_CONFIG") {
      return Ok(PathBuf::from(path));
    }
    let base = dirs::config_dir().context("could not locate the config directory")?;
    Ok(base.join("ainz/config.toml"))
  }

  pub async fn load() -> Result<Self> {
    let path = Self::path()?;
    if path.exists() || env::var_os("AINZ_CONFIG").is_some() {
      return Self::load_from(&path).await;
    }
    let legacy = dirs::config_dir()
      .context("could not locate the config directory")?
      .join("agentx/config.toml");
    if !legacy.exists() {
      return Self::load_from(&path).await;
    }
    let config = Self::load_from(&legacy).await?;
    config.save_to(&path).await?;
    Ok(config)
  }

  pub async fn load_from(path: &Path) -> Result<Self> {
    let mut config = match tokio::fs::read_to_string(path).await {
      Ok(text) => toml::from_str(&text)
        .with_context(|| format!("invalid configuration at {}", path.display()))?,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
      Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };

    if let Ok(endpoint) = env::var("AINZ_ENDPOINT") {
      config.endpoint = endpoint;
    }
    if let Ok(model) = env::var("AINZ_MODEL") {
      config.model = model;
    }
    if let Ok(provider) = env::var("AINZ_PROVIDER") {
      config.provider = Some(provider);
    }
    if let Ok(backend) = env::var("AINZ_MEMORY") {
      config.memory.backend = MemoryBackend::parse(&backend)?;
    }
    if let Ok(value) = env::var("AINZ_SYNAPSE") {
      config.synapse.enabled = matches!(value.trim(), "1" | "on" | "true" | "yes");
    }

    config.refresh_presets();
    Ok(config)
  }

  /// The first Claude Code preset asked for one buffered JSON result, so nothing reached the
  /// screen until the whole run was over. A profile still carrying exactly those arguments was
  /// written by setup, not by hand, so it is moved to the streaming ones.
  fn refresh_presets(&mut self) {
    const BUFFERED_CLAUDE: [&str; 8] = [
      "-p",
      "--output-format",
      "json",
      "--no-session-persistence",
      "--model",
      "{model}",
      "--permission-mode",
      "{permission}",
    ];
    for provider in self.providers.values_mut() {
      if provider.kind == ProviderKind::Process
        && provider.command.as_deref() == Some("claude")
        && provider.output == ProcessOutput::JsonResult
        && provider.args == BUFFERED_CLAUDE
      {
        let preset = ProviderConfig::claude_code();
        provider.args = preset.args;
        provider.output = preset.output;
      }
    }
  }

  pub async fn save(&self) -> Result<()> {
    self.save_to(&Self::path()?).await
  }

  pub async fn save_to(&self, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
      tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(self)?;
    tokio::fs::write(path, text)
      .await
      .with_context(|| format!("write {}", path.display()))
  }

  pub fn active_provider(&self) -> Result<ProviderConfig> {
    match &self.provider {
      Some(name) => self
        .providers
        .get(name)
        .cloned()
        .with_context(|| format!("provider {name} is not configured")),
      None => Ok(ProviderConfig::http(
        self.endpoint.clone(),
        self.api_key_env.clone(),
      )),
    }
  }

  pub fn api_key_for(&self, provider: &ProviderConfig) -> Result<Option<String>> {
    if provider.api_key_env.is_empty() {
      return Ok(None);
    }
    match env::var(&provider.api_key_env) {
      Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
      Ok(_) | Err(env::VarError::NotPresent) => Ok(None),
      Err(error) => Err(error).context("API key environment variable is not valid Unicode"),
    }
  }

  // the memory backend implies the integration, so `backend = "synapse"` cannot be a dead setting
  pub fn synapse_active(&self) -> bool {
    self.synapse.enabled || self.memory.backend == MemoryBackend::Synapse
  }

  pub fn mesh_active(&self) -> bool {
    self.synapse.mesh && self.synapse_active()
  }

  pub fn validate(&self) -> Result<()> {
    if self.model.trim().is_empty() {
      bail!(
        "no model configured; use `ainz providers use NAME MODEL`, set AINZ_MODEL, or set model in {}",
        Self::path()?.display()
      );
    }
    let provider = self.active_provider()?;
    provider.validate(self.provider.as_deref().unwrap_or("default"))?;
    if self.max_steps == 0 {
      bail!("max_steps must be greater than zero");
    }
    if self.compact_at_tokens == 0 || self.compact_at_tokens >= self.context_tokens {
      bail!("compact_at_tokens must be greater than zero and below context_tokens");
    }
    if self.preserve_messages < 2 {
      bail!("preserve_messages must be at least two");
    }
    if self.memory.backend != MemoryBackend::Off && self.memory.recall_limit == 0 {
      bail!("memory.recall_limit must be greater than zero");
    }
    Ok(())
  }
}
