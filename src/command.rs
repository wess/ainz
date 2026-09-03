use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use uuid::Uuid;

use ainz::{
  Config, HttpProvider, ImportKind, LocalSkills, McpProfile, McpServerConfig, McpTransport,
  MemoryBackend, PluginCatalog, ProcessOutput, PromptCatalog, ProviderConfig, ProviderKind,
  Session, SessionStore, SkillCatalog, import as importer, synapse,
};

#[derive(Subcommand)]
pub enum SessionsCommand {
  /// Write the session's active conversation path as Markdown, to --out or stdout.
  Export {
    id: Option<Uuid>,
    #[arg(long)]
    out: Option<PathBuf>,
  },
}

#[derive(Subcommand)]
pub enum McpCommand {
  Add {
    name: String,
    #[arg(long)]
    required: bool,
    #[arg(last = true, required = true, allow_hyphen_values = true)]
    command: Vec<String>,
  },
  Remove {
    name: String,
  },
}

#[derive(Subcommand)]
pub enum PluginCommand {
  List {
    #[arg(long)]
    json: bool,
  },
  Approve {
    name: String,
  },
  Revoke {
    name: String,
  },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ProviderPreset {
  Ollama,
  LiteLlm,
  Codex,
  ClaudeCode,
}

// the arguments themselves live with the config, so a saved profile can be compared against
// the preset it came from when its shape changes
pub(crate) fn preset_profile(preset: ProviderPreset) -> ProviderConfig {
  match preset {
    ProviderPreset::Ollama => ProviderConfig::ollama(),
    ProviderPreset::LiteLlm => ProviderConfig::lite_llm(),
    ProviderPreset::Codex => ProviderConfig::codex(),
    ProviderPreset::ClaudeCode => ProviderConfig::claude_code(),
  }
}

#[derive(Subcommand)]
pub enum SkillCommand {
  List {
    #[arg(long)]
    json: bool,
  },
  Proposed {
    #[arg(long)]
    json: bool,
  },
  Approve {
    name: String,
  },
  Reject {
    name: String,
  },
}

#[derive(Subcommand)]
pub enum MemoryCommand {
  List {
    #[arg(long, default_value_t = 10)]
    limit: usize,
    #[arg(long)]
    json: bool,
  },
  Search {
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    #[arg(long)]
    json: bool,
  },
  Add {
    #[arg(required = true, trailing_var_arg = true)]
    content: Vec<String>,
    #[arg(long)]
    global: bool,
    #[arg(long)]
    source: Option<String>,
  },
  Forget {
    id: String,
  },
  Backend {
    value: String,
  },
  Teach {
    state: String,
  },
}

#[derive(Subcommand)]
pub enum SynapseCommand {
  Status,
  Enable,
  Disable,
  Mesh { state: String },
}

#[derive(Subcommand)]
pub enum ProviderCommand {
  List {
    #[arg(long)]
    json: bool,
  },
  Add {
    name: String,
    #[arg(long, value_enum)]
    preset: Option<ProviderPreset>,
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long)]
    command: Option<String>,
    #[arg(long = "arg", allow_hyphen_values = true)]
    args: Vec<String>,
    #[arg(long, default_value = "AINZ_API_KEY")]
    api_key_env: String,
    #[arg(long = "known-model")]
    models: Vec<String>,
    #[arg(long)]
    json_result: bool,
    /// Read one JSON object per line as the command writes them, the way `claude -p
    /// --output-format stream-json` reports itself
    #[arg(long)]
    stream_json: bool,
  },
  Remove {
    name: String,
  },
  Use {
    name: String,
    model: Option<String>,
  },
}

#[derive(Subcommand)]
pub enum ModelCommand {
  List {
    provider: Option<String>,
    #[arg(long)]
    refresh: bool,
    #[arg(long)]
    json: bool,
  },
  Add {
    provider: String,
    model: String,
  },
  Remove {
    provider: String,
    model: String,
  },
}

pub async fn load_session(workspace: &Path, id: Option<Uuid>) -> Result<Session> {
  let store = SessionStore::default_store()?;
  let id = match id {
    Some(id) => id,
    None => {
      store
        .list()
        .await?
        .into_iter()
        .find(|session| session.workspace == workspace)
        .context("no saved session for this workspace")?
        .id
    }
  };
  let session = store.load(id).await?;
  if session.workspace != workspace {
    anyhow::bail!("session {id} belongs to a different workspace");
  }
  Ok(session)
}

pub async fn list_sessions(workspace: &Path, json: bool) -> Result<()> {
  let sessions: Vec<_> = SessionStore::default_store()?
    .list()
    .await?
    .into_iter()
    .filter(|session| session.workspace == workspace)
    .collect();
  if json {
    println!("{}", serde_json::to_string_pretty(&sessions)?);
  } else {
    for session in sessions {
      println!(
        "{}  {} nodes  {}{}",
        session.id,
        session.nodes,
        session.updated_at,
        session
          .parent_id
          .map_or_else(String::new, |parent| format!("  child of {parent}"))
      );
    }
  }
  Ok(())
}

pub async fn export_session(workspace: &Path, id: Option<Uuid>, out: Option<&Path>) -> Result<()> {
  let session = load_session(workspace, id).await?;
  let markdown = session.export_markdown()?;
  match out {
    Some(path) => tokio::fs::write(path, &markdown)
      .await
      .with_context(|| format!("write {}", path.display()))?,
    None => print!("{markdown}"),
  }
  Ok(())
}

pub async fn search_sessions(workspace: &Path, query: &str, json: bool) -> Result<()> {
  let matches = SessionStore::default_store()?
    .search(query, Some(workspace), 20)
    .await?;
  if json {
    println!("{}", serde_json::to_string_pretty(&matches)?);
  } else if matches.is_empty() {
    println!("no earlier session mentioned that");
  } else {
    for found in matches {
      println!("{}  {} of the terms", found.id, found.score);
      for excerpt in found.excerpts {
        println!("    {excerpt}");
      }
    }
  }
  Ok(())
}

pub async fn skills(
  workspace: &Path,
  config: &Config,
  command: Option<SkillCommand>,
  json: bool,
) -> Result<()> {
  match command {
    None | Some(SkillCommand::List { json: false }) if !json => list_skills(workspace, json).await,
    None | Some(SkillCommand::List { .. }) => list_skills(workspace, true).await,
    Some(SkillCommand::Proposed { json }) => {
      if config.memory.backend == MemoryBackend::Synapse {
        println!("proposed skills live in Synapse: synapse skill proposed");
        return Ok(());
      }
      let proposed = LocalSkills::new()?.proposed().await?;
      if json {
        let rows: Vec<_> = proposed
          .iter()
          .map(|skill| {
            serde_json::json!({
              "name": skill.name, "description": skill.description, "path": skill.path,
            })
          })
          .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
      } else if proposed.is_empty() {
        println!("no proposed skills");
      } else {
        for skill in proposed {
          println!("{}  {}", skill.name, skill.description);
        }
      }
      Ok(())
    }
    Some(SkillCommand::Approve { name }) => {
      if config.memory.backend == MemoryBackend::Synapse {
        println!("approve it in Synapse: synapse skill approve {name}");
        return Ok(());
      }
      let path = LocalSkills::new()?.approve(&name).await?;
      println!("approved {name} at {}", path.display());
      Ok(())
    }
    Some(SkillCommand::Reject { name }) => {
      if config.memory.backend == MemoryBackend::Synapse {
        println!("reject it in Synapse: synapse skill reject {name}");
        return Ok(());
      }
      LocalSkills::new()?.reject(&name).await?;
      println!("rejected {name}");
      Ok(())
    }
  }
}

pub async fn memory(workspace: &Path, config: &mut Config, command: MemoryCommand) -> Result<()> {
  match command {
    MemoryCommand::Backend { value } => {
      config.memory.backend = MemoryBackend::parse(&value)?;
      if config.memory.backend == MemoryBackend::Synapse {
        config.synapse.enabled = true;
      }
      config.save().await?;
      println!("memory backend {}", config.memory.backend.label());
      return Ok(());
    }
    MemoryCommand::Teach { state } => {
      config.memory.teach = switch(&state)?;
      config.save().await?;
      println!("self-improvement {}", state.trim());
      return Ok(());
    }
    _ => {}
  }
  let store = crate::app::memory_store(workspace, config).await?;
  if store.is_off() {
    println!("memory is off; turn it on with `ainz memory backend local`");
    return Ok(());
  }
  match command {
    MemoryCommand::List { limit, json } | MemoryCommand::Search { limit, json, .. } => {
      let query = match &command {
        MemoryCommand::Search { query, .. } => query.clone(),
        _ => String::new(),
      };
      let records = store.recall(&query, limit).await?;
      if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
      } else if records.is_empty() {
        println!("no memories matched");
      } else {
        for record in records {
          println!("{}  {}", record.id, record.summary(96));
        }
      }
    }
    MemoryCommand::Add {
      content,
      global,
      source,
    } => {
      let scope = if global { "global" } else { "project" };
      let message = store
        .remember(&content.join(" "), source.as_deref(), scope, &[])
        .await?;
      println!("{message}");
    }
    MemoryCommand::Forget { id } => println!("{}", store.forget(&id).await?),
    MemoryCommand::Backend { .. } | MemoryCommand::Teach { .. } => {}
  }
  Ok(())
}

/// Lists what other tools on this machine already have, and copies over what is chosen.
pub async fn import(
  workspace: &Path,
  config: &Config,
  names: &[String],
  kind: Option<&str>,
  all: bool,
  json: bool,
) -> Result<()> {
  let memory = crate::app::memory_store(workspace, config).await?;
  let kind = kind.map(ImportKind::parse).transpose()?;
  let found: Vec<_> = importer::discover(workspace, &memory)
    .await?
    .into_iter()
    .filter(|candidate| kind.is_none_or(|kind| candidate.kind == kind))
    .collect();

  if !all && names.is_empty() {
    if json {
      let rows: Vec<_> = found
        .iter()
        .map(|candidate| {
          serde_json::json!({
            "kind": candidate.kind.label(), "name": candidate.name,
            "origin": candidate.origin, "detail": candidate.detail,
            "credentials": candidate.secrets, "present": candidate.present,
          })
        })
        .collect();
      println!("{}", serde_json::to_string_pretty(&rows)?);
      return Ok(());
    }
    if found.is_empty() {
      println!("nothing to import; Ainz already reads what these tools have");
      return Ok(());
    }
    for candidate in &found {
      println!(
        "{:<10} {:<24} {}{}{}",
        candidate.kind.label(),
        candidate.name,
        candidate.origin,
        if candidate.present {
          "  already available"
        } else {
          ""
        },
        if candidate.secrets {
          "  carries credentials"
        } else {
          ""
        }
      );
    }
    println!("\nimport with `ainz import NAME…` or `ainz import --all`");
    return Ok(());
  }

  let chosen: Vec<_> = found
    .into_iter()
    .filter(|candidate| {
      if names.is_empty() {
        !candidate.present
      } else {
        names.iter().any(|name| name == &candidate.name)
      }
    })
    .collect();
  if chosen.is_empty() {
    println!("nothing matched");
    return Ok(());
  }
  for line in importer::import(&chosen, &memory).await? {
    println!("{line}");
  }
  Ok(())
}

pub async fn synapse(config: &mut Config, command: Option<SynapseCommand>) -> Result<()> {
  match command {
    Some(SynapseCommand::Enable) => {
      config.synapse.enabled = true;
      config.save().await?;
    }
    Some(SynapseCommand::Disable) => {
      config.synapse.enabled = false;
      config.synapse.mesh = false;
      if config.memory.backend == MemoryBackend::Synapse {
        config.memory.backend = MemoryBackend::Local;
      }
      config.save().await?;
    }
    Some(SynapseCommand::Mesh { state }) => {
      config.synapse.mesh = switch(&state)?;
      if config.synapse.mesh {
        config.synapse.enabled = true;
      }
      config.save().await?;
    }
    Some(SynapseCommand::Status) | None => {}
  }
  println!(
    "synapse   {}",
    if config.synapse.enabled { "on" } else { "off" }
  );
  println!(
    "binary    {}",
    synapse::binary(&config.synapse)
      .map(|path| path.display().to_string())
      .unwrap_or_else(|| format!("not installed — {}", synapse::SITE))
  );
  println!(
    "mesh      {}",
    if config.synapse.mesh { "on" } else { "off" }
  );
  println!("memory    {}", config.memory.backend.label());
  println!(
    "learn     {}",
    if config.memory.teach { "on" } else { "off" }
  );
  Ok(())
}

fn switch(state: &str) -> Result<bool> {
  match state.trim() {
    "on" | "true" | "yes" => Ok(true),
    "off" | "false" | "no" => Ok(false),
    other => anyhow::bail!("expected on or off, got {other}"),
  }
}

pub async fn list_skills(workspace: &Path, json: bool) -> Result<()> {
  let plugins = PluginCatalog::discover(workspace).await?;
  let catalog =
    SkillCatalog::discover_with_roots(workspace, &plugins.approved_skill_roots()).await?;
  if json {
    let rows: Vec<_> = catalog
      .skills
      .iter()
      .map(|skill| {
        serde_json::json!({
          "name": skill.name, "description": skill.description, "path": skill.path,
        })
      })
      .collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
  } else {
    for skill in catalog.skills {
      println!("{}  {}", skill.name, skill.description);
    }
  }
  Ok(())
}

pub async fn prompts(
  workspace: &Path,
  name: Option<&str>,
  args: &[String],
  json: bool,
) -> Result<()> {
  let catalog = PromptCatalog::discover(workspace).await?;
  if let Some(name) = name {
    println!("{}", catalog.expand(name, args).await?);
  } else if json {
    let rows: Vec<_> = catalog
      .prompts
      .iter()
      .map(|prompt| {
        serde_json::json!({
          "name": prompt.name, "description": prompt.description, "path": prompt.path,
        })
      })
      .collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
  } else {
    for prompt in catalog.prompts {
      println!("{}  {}", prompt.name, prompt.description);
    }
  }
  Ok(())
}

pub async fn usage(workspace: &Path, json: bool) -> Result<()> {
  let sessions: Vec<_> = SessionStore::default_store()?
    .list()
    .await?
    .into_iter()
    .filter(|session| session.workspace == workspace)
    .collect();
  let input_tokens: u64 = sessions
    .iter()
    .map(|session| session.usage.input_tokens)
    .sum();
  let output_tokens: u64 = sessions
    .iter()
    .map(|session| session.usage.output_tokens)
    .sum();
  // only a process provider that reports its own spend sets cost_usd; sum what's known and
  // say nothing when none of these sessions used one
  let cost_usd = sessions
    .iter()
    .filter_map(|session| session.usage.cost_usd)
    .fold(None, |total: Option<f64>, cost| {
      Some(total.unwrap_or(0.0) + cost)
    });
  if json {
    let mut body = serde_json::json!({
      "sessions": sessions.len(), "input_tokens": input_tokens,
      "output_tokens": output_tokens, "total_tokens": input_tokens + output_tokens,
    });
    if let Some(cost_usd) = cost_usd {
      body["cost_usd"] = serde_json::json!(cost_usd);
    }
    println!("{}", serde_json::to_string_pretty(&body)?);
  } else {
    println!("sessions       {}", sessions.len());
    println!("input tokens   {input_tokens}");
    println!("output tokens  {output_tokens}");
    println!("total tokens   {}", input_tokens + output_tokens);
    if let Some(cost_usd) = cost_usd {
      println!("cost           ${cost_usd:.2}");
    }
  }
  Ok(())
}

pub async fn mcp(command: Option<McpCommand>, json: bool) -> Result<()> {
  let mut profile = McpProfile::load().await?;
  match command {
    Some(McpCommand::Add {
      name,
      required,
      command,
    }) => {
      if !ainz::mcp::valid_name(&name) {
        anyhow::bail!("MCP server name {name:?} may only use letters, digits, '.', '_' and '-'");
      }
      if profile.servers.contains_key(&name) {
        anyhow::bail!("MCP server {name} is already configured");
      }
      profile.servers.insert(
        name.clone(),
        McpServerConfig {
          transport: McpTransport::Stdio,
          command,
          url: None,
          header_env: Default::default(),
          headers: Default::default(),
          env: Default::default(),
          cwd: None,
          enabled: true,
          required,
          timeout_ms: 30_000,
        },
      );
      profile.save().await?;
      println!("added MCP server {name}");
      return Ok(());
    }
    Some(McpCommand::Remove { name }) => {
      if profile.servers.remove(&name).is_none() {
        anyhow::bail!("MCP server {name} is not configured");
      }
      profile.save().await?;
      println!("removed MCP server {name}");
      return Ok(());
    }
    None => {}
  }
  if json {
    println!("{}", serde_json::to_string_pretty(&redacted(profile))?);
  } else {
    println!("profile  {}", McpProfile::path()?.display());
    for (name, server) in profile.servers {
      println!(
        "{}  {}  {}",
        name,
        if server.enabled {
          "enabled"
        } else {
          "disabled"
        },
        if server.required {
          "required"
        } else {
          "optional"
        }
      );
    }
  }
  Ok(())
}

// header and env values are the only places a secret can sit in the profile
fn redacted(mut profile: McpProfile) -> McpProfile {
  for server in profile.servers.values_mut() {
    for value in server.headers.values_mut().chain(server.env.values_mut()) {
      *value = "<redacted>".into();
    }
  }
  profile
}

pub async fn plugins(workspace: &Path, command: PluginCommand) -> Result<()> {
  let mut catalog = PluginCatalog::discover(workspace).await?;
  match command {
    PluginCommand::List { json } => print_plugins(&catalog, json)?,
    PluginCommand::Approve { name } => {
      let plugin = catalog
        .plugins
        .iter()
        .find(|plugin| plugin.manifest.plugin.name == name)
        .with_context(|| format!("plugin {name} was not found"))?;
      // what is being trusted goes on the record before the grant is written
      println!("{}", describe_plugin(plugin));
      catalog.approve(&name).await?;
      println!("approved {name}; changing pinned plugin content revokes this approval");
    }
    PluginCommand::Revoke { name } => {
      catalog.revoke(&name).await?;
      println!("revoked {name}");
    }
  }
  Ok(())
}

pub async fn providers(config: &mut Config, command: ProviderCommand) -> Result<()> {
  match command {
    ProviderCommand::List { json } => print_providers(config, json)?,
    ProviderCommand::Add {
      name,
      preset,
      endpoint,
      command,
      args,
      api_key_env,
      models,
      json_result,
      stream_json,
    } => {
      if config.providers.contains_key(&name) {
        anyhow::bail!("provider {name} already exists");
      }
      let mut profile = match preset {
        Some(_) if endpoint.is_some() || command.is_some() || !args.is_empty() => {
          anyhow::bail!("--preset cannot be combined with --endpoint, --command, or --arg")
        }
        Some(preset) => preset_profile(preset),
        None => match (endpoint, command) {
          (Some(endpoint), None) => ProviderConfig::http(endpoint, api_key_env),
          (None, Some(command)) => ProviderConfig::process(
            command,
            args,
            match (stream_json, json_result) {
              (true, _) => ProcessOutput::StreamJson,
              (false, true) => ProcessOutput::JsonResult,
              (false, false) => ProcessOutput::Text,
            },
          ),
          (Some(_), Some(_)) => anyhow::bail!("choose either --endpoint or --command"),
          (None, None) => anyhow::bail!("provider requires --preset, --endpoint, or --command"),
        },
      };
      profile.models.extend(models);
      profile.models.sort();
      profile.models.dedup();
      profile.validate(&name)?;
      config.providers.insert(name.clone(), profile);
      config.save().await?;
      println!("added provider {name}");
    }
    ProviderCommand::Remove { name } => {
      if config.provider.as_deref() == Some(&name) {
        anyhow::bail!("provider {name} is active; select another provider first");
      }
      config
        .providers
        .remove(&name)
        .with_context(|| format!("provider {name} is not configured"))?;
      config.save().await?;
      println!("removed provider {name}");
    }
    ProviderCommand::Use { name, model } => {
      let profile = config
        .providers
        .get_mut(&name)
        .with_context(|| format!("provider {name} is not configured"))?;
      if let Some(model) = model {
        if !profile.models.contains(&model) {
          profile.models.push(model.clone());
          profile.models.sort();
        }
        config.model = model;
      } else if config.model.is_empty()
        || (!profile.models.is_empty() && !profile.models.contains(&config.model))
      {
        config.model = profile
          .models
          .first()
          .cloned()
          .context("provider has no models; pass a model or add one with `ainz models add`")?;
      }
      config.provider = Some(name.clone());
      config.save().await?;
      println!("using {name} · {}", config.model);
    }
  }
  Ok(())
}

pub async fn models(config: &mut Config, command: ModelCommand) -> Result<()> {
  match command {
    ModelCommand::List {
      provider,
      refresh,
      json,
    } => {
      let name = provider
        .or_else(|| config.provider.clone())
        .context("provider name is required when no provider is active")?;
      let profile = config
        .providers
        .get(&name)
        .cloned()
        .with_context(|| format!("provider {name} is not configured"))?;
      let models = if refresh {
        if profile.kind != ProviderKind::Http {
          anyhow::bail!("provider {name} does not support model discovery");
        }
        let provider = HttpProvider::new(
          profile
            .endpoint
            .clone()
            .context("HTTP provider requires an endpoint")?,
          config.model.clone(),
          config.api_key_for(&profile)?,
          config.provider_retries,
        )?;
        let models = provider.models().await?;
        if let Some(profile) = config.providers.get_mut(&name) {
          profile.models = models.clone();
        }
        config.save().await?;
        models
      } else {
        profile.models
      };
      if json {
        println!("{}", serde_json::to_string_pretty(&models)?);
      } else {
        for model in models {
          println!(
            "{}{}",
            model,
            if config.provider.as_deref() == Some(&name) && config.model == model {
              "  active"
            } else {
              ""
            }
          );
        }
      }
    }
    ModelCommand::Add { provider, model } => {
      let profile = config
        .providers
        .get_mut(&provider)
        .with_context(|| format!("provider {provider} is not configured"))?;
      if !profile.models.contains(&model) {
        profile.models.push(model.clone());
        profile.models.sort();
      }
      config.save().await?;
      println!("added model {model} to {provider}");
    }
    ModelCommand::Remove { provider, model } => {
      if config.provider.as_deref() == Some(&provider) && config.model == model {
        anyhow::bail!("model {model} is active; select another model first");
      }
      let profile = config
        .providers
        .get_mut(&provider)
        .with_context(|| format!("provider {provider} is not configured"))?;
      let before = profile.models.len();
      profile.models.retain(|entry| entry != &model);
      if profile.models.len() == before {
        anyhow::bail!("model {model} is not configured for {provider}");
      }
      config.save().await?;
      println!("removed model {model} from {provider}");
    }
  }
  Ok(())
}

fn print_providers(config: &Config, json: bool) -> Result<()> {
  if json {
    let rows: Vec<_> = config
      .providers
      .iter()
      .map(|(name, provider)| {
        serde_json::json!({
          "name": name,
          "kind": provider.kind,
          "active": config.provider.as_deref() == Some(name),
          "models": provider.models,
        })
      })
      .collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
  } else {
    for (name, provider) in &config.providers {
      println!(
        "{}  {:?}  {} models{}",
        name,
        provider.kind,
        provider.models.len(),
        if config.provider.as_deref() == Some(name) {
          "  active"
        } else {
          ""
        }
      );
    }
  }
  Ok(())
}

fn print_plugins(catalog: &PluginCatalog, json: bool) -> Result<()> {
  if json {
    let rows: Vec<_> = catalog
      .plugins
      .iter()
      .map(|plugin| {
        serde_json::json!({
          "name": plugin.manifest.plugin.name, "version": plugin.manifest.plugin.version,
          "approved": plugin.approved, "path": plugin.path,
          "format": format!("{:?}", plugin.format),
          "capabilities": plugin.manifest.capabilities,
        })
      })
      .collect();
    println!("{}", serde_json::to_string_pretty(&rows)?);
  } else {
    for plugin in &catalog.plugins {
      println!("{}", describe_plugin(plugin));
    }
    for issue in &catalog.issues {
      println!("invalid  {issue}");
    }
  }
  Ok(())
}

fn describe_plugin(plugin: &ainz::plugin::DiscoveredPlugin) -> String {
  let manifest = &plugin.manifest;
  let artifact = plugin
    .artifact()
    .map(|path| path.display().to_string())
    .unwrap_or_else(|| "skills and MCP servers".into());
  let capabilities = manifest
    .capabilities
    .iter()
    .map(|capability| format!("{capability:?}").to_lowercase())
    .collect::<Vec<_>>()
    .join(",");
  format!(
    "{} {}  {}  {:?} {:?}  {}  [{capabilities}]  {}",
    manifest.plugin.name,
    manifest.plugin.version,
    if plugin.approved {
      "approved"
    } else {
      "pending"
    },
    plugin.format,
    manifest.runtime.kind,
    artifact,
    plugin.root().display()
  )
}

pub async fn doctor(workspace: &Path, config: &Config) -> Result<()> {
  println!("workspace  {}", workspace.display());
  println!("config     {}", Config::path()?.display());
  println!(
    "provider   {}",
    config.provider.as_deref().unwrap_or("legacy HTTP")
  );
  let provider = config.active_provider()?;
  match provider.kind {
    ProviderKind::Http => println!(
      "endpoint   {}",
      provider.endpoint.as_deref().unwrap_or("not configured")
    ),
    ProviderKind::Process => println!(
      "command    {}",
      provider.command.as_deref().unwrap_or("not configured")
    ),
  }
  println!(
    "model      {}",
    if config.model.is_empty() {
      "not configured"
    } else {
      &config.model
    }
  );
  println!(
    "permissions {}{}",
    match config.permissions {
      ainz::PermissionMode::Ask => "ask",
      ainz::PermissionMode::Auto => "auto",
      ainz::PermissionMode::ReadOnly => "read-only",
    },
    if config.yeet { " · yeet" } else { "" }
  );
  println!("sessions   {}", SessionStore::default_path()?.display());
  let catalog = PluginCatalog::discover(workspace).await?;
  println!(
    "plugins    {} discovered, {} approved",
    catalog.plugins.len(),
    catalog
      .plugins
      .iter()
      .filter(|plugin| plugin.approved)
      .count()
  );
  for issue in &catalog.issues {
    println!("           invalid  {issue}");
  }
  let skills =
    SkillCatalog::discover_with_roots(workspace, &catalog.approved_skill_roots()).await?;
  println!("skills     {} discovered", skills.skills.len());
  let prompts = PromptCatalog::discover(workspace).await?;
  println!("prompts    {} discovered", prompts.prompts.len());
  println!(
    "memory     {}{}",
    config.memory.backend.label(),
    if config.memory.teach {
      " · self-improvement on"
    } else {
      ""
    }
  );
  let binary = synapse::binary(&config.synapse);
  println!(
    "synapse    {}{}",
    if config.synapse.enabled {
      "enabled"
    } else {
      "disabled"
    },
    match (&binary, config.synapse.mesh) {
      (Some(path), true) => format!(" · mesh on · {}", path.display()),
      (Some(path), false) => format!(" · {}", path.display()),
      (None, _) => format!(" · not installed — {}", synapse::SITE),
    }
  );
  Ok(())
}
