use std::path::Path;

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use uuid::Uuid;

use agentx::{
  Config, HttpProvider, McpProfile, McpServerConfig, McpTransport, PluginCatalog, ProcessOutput,
  PromptCatalog, ProviderConfig, ProviderKind, Session, SessionStore, SkillCatalog,
  session::workspace_matches,
};

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
  Codex,
  ClaudeCode,
}

pub(crate) fn preset_profile(preset: ProviderPreset) -> ProviderConfig {
  match preset {
    ProviderPreset::Ollama => ProviderConfig::http("http://127.0.0.1:11434/v1", ""),
    ProviderPreset::Codex => ProviderConfig::process(
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
    ),
    ProviderPreset::ClaudeCode => ProviderConfig::process(
      "claude",
      strings(&[
        "-p",
        "--output-format",
        "json",
        "--no-session-persistence",
        "--model",
        "{model}",
        "--permission-mode",
        "{permission}",
      ]),
      ProcessOutput::JsonResult,
    ),
  }
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
    #[arg(long, default_value = "AGENTX_API_KEY")]
    api_key_env: String,
    #[arg(long = "known-model")]
    models: Vec<String>,
    #[arg(long)]
    json_result: bool,
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
  if let Some(id) = id {
    let session = store.load(id).await?;
    if !workspace_matches(&session, workspace) {
      anyhow::bail!("session {id} belongs to a different workspace");
    }
    return Ok(session);
  }
  store
    .list()
    .await?
    .into_iter()
    .find(|session| workspace_matches(session, workspace))
    .context("no saved session for this workspace")
}

pub async fn list_sessions(workspace: &Path, json: bool) -> Result<()> {
  let sessions: Vec<_> = SessionStore::default_store()?
    .list()
    .await?
    .into_iter()
    .filter(|session| workspace_matches(session, workspace))
    .collect();
  if json {
    println!("{}", serde_json::to_string_pretty(&sessions)?);
  } else {
    for session in sessions {
      println!(
        "{}  {} nodes  {}{}",
        session.id,
        session.nodes.len(),
        session.updated_at,
        session
          .parent_id
          .map_or_else(String::new, |parent| format!("  child of {parent}"))
      );
    }
  }
  Ok(())
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
    .filter(|session| workspace_matches(session, workspace))
    .collect();
  let input_tokens: u64 = sessions
    .iter()
    .map(|session| session.usage.input_tokens)
    .sum();
  let output_tokens: u64 = sessions
    .iter()
    .map(|session| session.usage.output_tokens)
    .sum();
  if json {
    println!(
      "{}",
      serde_json::to_string_pretty(&serde_json::json!({
        "sessions": sessions.len(), "input_tokens": input_tokens,
        "output_tokens": output_tokens, "total_tokens": input_tokens + output_tokens,
      }))?
    );
  } else {
    println!("sessions       {}", sessions.len());
    println!("input tokens   {input_tokens}");
    println!("output tokens  {output_tokens}");
    println!("total tokens   {}", input_tokens + output_tokens);
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
    println!("{}", serde_json::to_string_pretty(&profile)?);
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

pub async fn plugins(workspace: &Path, command: PluginCommand) -> Result<()> {
  let mut catalog = PluginCatalog::discover(workspace).await?;
  match command {
    PluginCommand::List { json } => print_plugins(&catalog, json)?,
    PluginCommand::Approve { name } => {
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
            if json_result {
              ProcessOutput::JsonResult
            } else {
              ProcessOutput::Text
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
          .context("provider has no models; pass a model or add one with `agentx models add`")?;
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
        )?;
        let models = provider.models().await?;
        config.providers.get_mut(&name).unwrap().models = models.clone();
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

fn strings(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_string()).collect()
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
      println!(
        "{} {}  {}  {:?}  {}",
        plugin.manifest.plugin.name,
        plugin.manifest.plugin.version,
        if plugin.approved {
          "approved"
        } else {
          "pending"
        },
        plugin.format,
        plugin.path.display()
      );
    }
  }
  Ok(())
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
  Ok(())
}
