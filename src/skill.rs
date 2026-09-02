use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::fs;

use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

#[derive(Clone, Debug)]
pub struct Skill {
  pub name: String,
  pub description: String,
  pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct SkillCatalog {
  pub skills: Vec<Skill>,
}

impl SkillCatalog {
  pub async fn discover(workspace: &Path) -> Result<Self> {
    Self::discover_with_roots(workspace, &[]).await
  }

  pub async fn discover_with_roots(workspace: &Path, extra_roots: &[PathBuf]) -> Result<Self> {
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
      roots.push(config.join("struts/skills"));
      roots.push(config.join("agentx/skills"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    for path in ancestors {
      roots.push(path.join("skills"));
      roots.push(path.join(".agents/skills"));
      roots.push(path.join(".struts/skills"));
      roots.push(path.join(".agentx/skills"));
    }
    roots.extend_from_slice(extra_roots);

    let mut skills = BTreeMap::new();
    for root in roots {
      for skill in discover_root(&root).await? {
        skills.insert(skill.name.clone(), skill);
      }
    }
    Ok(Self {
      skills: skills.into_values().collect(),
    })
  }

  pub fn tool(&self) -> Arc<dyn Tool> {
    Arc::new(SkillTool {
      skills: self.skills.clone(),
    })
  }
}

async fn discover_root(root: &Path) -> Result<Vec<Skill>> {
  let mut entries = match fs::read_dir(root).await {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(error).with_context(|| format!("read {}", root.display())),
  };
  let mut skills = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    let path = entry.path().join("SKILL.md");
    let text = match fs::read_to_string(&path).await {
      Ok(text) => text,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let fallback = entry.file_name().to_string_lossy().into_owned();
    let (name, description) = metadata(&text, &fallback);
    if name.trim().is_empty() {
      continue;
    }
    skills.push(Skill {
      name,
      description,
      path,
    });
  }
  Ok(skills)
}

fn metadata(text: &str, fallback: &str) -> (String, String) {
  let Some(rest) = text.strip_prefix("---\n") else {
    return (fallback.into(), first_heading(text));
  };
  let Some(end) = rest.find("\n---") else {
    return (fallback.into(), first_heading(text));
  };
  let mut name = fallback.to_string();
  let mut description = String::new();
  for line in rest[..end].lines() {
    if let Some(value) = line.strip_prefix("name:") {
      name = clean(value);
    } else if let Some(value) = line.strip_prefix("description:") {
      description = clean(value);
    }
  }
  (name, description)
}

fn clean(value: &str) -> String {
  value.trim().trim_matches(['"', '\'']).to_string()
}

fn first_heading(text: &str) -> String {
  text
    .lines()
    .find_map(|line| line.strip_prefix("# "))
    .unwrap_or_default()
    .to_string()
}

struct SkillTool {
  skills: Vec<Skill>,
}

#[derive(Deserialize)]
struct SkillArgs {
  name: String,
}

#[async_trait]
impl Tool for SkillTool {
  fn spec(&self) -> ToolSpec {
    let available = self
      .skills
      .iter()
      .map(|skill| {
        if skill.description.is_empty() {
          skill.name.clone()
        } else {
          format!("{}: {}", skill.name, skill.description)
        }
      })
      .collect::<Vec<_>>()
      .join("; ");
    ToolSpec {
      name: "skill".into(),
      description: format!("Load one skill's instructions on demand. Available: {available}"),
      parameters: json!({
        "type": "object", "properties": {"name": {"type": "string"}},
        "required": ["name"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Read
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let args: SkillArgs = serde_json::from_value(arguments)?;
    let skill = self
      .skills
      .iter()
      .find(|skill| skill.name == args.name)
      .with_context(|| format!("skill {} was not found", args.name))?;
    let text = fs::read_to_string(&skill.path)
      .await
      .with_context(|| format!("read {}", skill.path.display()))?;
    if text.trim().is_empty() {
      bail!("skill {} is empty", skill.name);
    }
    Ok(truncate(text, context.max_output_bytes))
  }
}
