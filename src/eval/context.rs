use std::path::Path;

// dependencies
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::fs;

// project
use crate::{Config, LocalSkills, SkillCatalog};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LoadedSkill {
  pub name: String,
  #[serde(default)]
  pub description: String,
  pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvalContext {
  pub config: Config,
  pub skills: Vec<LoadedSkill>,
  pub instructions: String,
}

impl EvalContext {
  pub async fn freeze(workspace: &Path, config: &Config, root: &Path) -> Result<Self> {
    let mut context = Self {
      config: config.clone(),
      skills: Vec::new(),
      instructions: crate::instruction::load(root).await?,
    };
    context.config.improve.enabled = false;
    context.config.hooks.clear();
    context.config.memory.backend = crate::MemoryBackend::Off;
    context.config.permissions = crate::PermissionMode::Auto;
    context.config.yeet = false;
    context.config.max_steps = 6;
    context.config.max_output_bytes = 8192;
    context.config.budget_usd = Some(
      config
        .budget_usd
        .unwrap_or(config.improve.budget_usd)
        .min(config.improve.budget_usd),
    );
    context.skills = installed(workspace).await?;
    fs::create_dir_all(root).await?;
    fs::write(
      root.join("context.json"),
      serde_json::to_vec_pretty(&context)?,
    )
    .await?;
    Ok(context)
  }

  pub async fn hash(root: &Path) -> Result<String> {
    Ok(format!(
      "{:x}",
      Sha256::digest(fs::read(root.join("context.json")).await?)
    ))
  }

  pub async fn load(root: &Path) -> Result<Self> {
    Ok(serde_json::from_slice(
      &fs::read(root.join("context.json")).await?,
    )?)
  }

  pub async fn verify_skills(&self, workspace: &Path) -> Result<()> {
    ensure!(
      self.skills == installed(workspace).await?,
      "installed skill context changed since behavioral evaluation; run a new experiment"
    );
    Ok(())
  }

  pub async fn verify_instructions(&self, root: &Path) -> Result<()> {
    ensure!(
      self.instructions == crate::instruction::load(root).await?,
      "instruction context changed since behavioral evaluation; run a new experiment"
    );
    Ok(())
  }

  pub fn verify_runtime(&self, config: &Config) -> Result<()> {
    ensure!(
      runtime(&self.config)? == runtime(config)?,
      "model or runtime settings changed since behavioral evaluation; run a new experiment"
    );
    Ok(())
  }
}

async fn installed(workspace: &Path) -> Result<Vec<LoadedSkill>> {
  let catalog = SkillCatalog::discover(workspace).await?;
  ensure!(
    catalog.issues.is_empty(),
    "could not freeze the complete skill catalog"
  );
  let mut bytes = 0;
  let mut skills = Vec::new();
  for skill in catalog.skills {
    let text = fs::read_to_string(&skill.path).await?;
    bytes += text.len();
    ensure!(
      bytes <= 512 * 1024,
      "evaluation skill context exceeds 512 KiB"
    );
    skills.push(LoadedSkill {
      name: skill.name,
      description: skill.description,
      text,
    });
  }
  Ok(skills)
}

fn runtime(config: &Config) -> Result<Value> {
  let fallback: Vec<_> = config
    .fallback_providers
    .iter()
    .map(|name| json!({"name": name, "config": config.providers.get(name)}))
    .collect();
  Ok(
    json!({"model": config.model, "provider": config.active_provider()?,
    "fallback": fallback, "context_tokens": config.context_tokens,
    "compact_at_tokens": config.compact_at_tokens, "preserve_messages": config.preserve_messages,
    "rules": config.rules}),
  )
}

pub async fn proposed(root: &Path) -> Result<Vec<LoadedSkill>> {
  let mut skills = Vec::new();
  for skill in LocalSkills::with_root(root.join("skills"))
    .proposed()
    .await?
  {
    skills.push(LoadedSkill {
      name: skill.name,
      description: skill.description,
      text: fs::read_to_string(skill.path).await?,
    });
  }
  Ok(skills)
}
