use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tokio::fs;

#[derive(Clone, Debug)]
pub struct PromptTemplate {
  pub name: String,
  pub description: String,
  pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct PromptCatalog {
  pub prompts: Vec<PromptTemplate>,
}

impl PromptCatalog {
  pub async fn discover(workspace: &Path) -> Result<Self> {
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
      roots.push(config.join("struts/prompts"));
      roots.push(config.join("agentx/prompts"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    for path in ancestors {
      roots.push(path.join(".struts/prompts"));
      roots.push(path.join(".agentx/prompts"));
    }

    let mut prompts = BTreeMap::new();
    for root in roots {
      for prompt in discover_root(&root).await? {
        prompts.insert(prompt.name.clone(), prompt);
      }
    }
    Ok(Self {
      prompts: prompts.into_values().collect(),
    })
  }

  pub async fn expand(&self, name: &str, args: &[String]) -> Result<String> {
    let prompt = self
      .prompts
      .iter()
      .find(|prompt| prompt.name == name)
      .with_context(|| format!("prompt template {name} was not found"))?;
    let text = fs::read_to_string(&prompt.path).await?;
    let body = strip_frontmatter(&text);
    let mut output = body.replace("{{args}}", &args.join(" "));
    for (index, value) in args.iter().enumerate() {
      output = output.replace(&format!("{{{{{}}}}}", index + 1), value);
    }
    Ok(output)
  }
}

async fn discover_root(root: &Path) -> Result<Vec<PromptTemplate>> {
  let mut entries = match fs::read_dir(root).await {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(error).with_context(|| format!("read {}", root.display())),
  };
  let mut prompts = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    let path = entry.path();
    if path.extension().is_none_or(|extension| extension != "md") {
      continue;
    }
    let text = fs::read_to_string(&path).await?;
    let fallback = path
      .file_stem()
      .unwrap_or_default()
      .to_string_lossy()
      .into_owned();
    let (name, description) = metadata(&text, &fallback);
    if !name.is_empty() {
      prompts.push(PromptTemplate {
        name,
        description,
        path,
      });
    }
  }
  Ok(prompts)
}

fn metadata(text: &str, fallback: &str) -> (String, String) {
  let Some(rest) = text.strip_prefix("---\n") else {
    return (fallback.into(), String::new());
  };
  let Some(end) = rest.find("\n---") else {
    return (fallback.into(), String::new());
  };
  let mut name = fallback.to_string();
  let mut description = String::new();
  for line in rest[..end].lines() {
    if let Some(value) = line.strip_prefix("name:") {
      name = clean(value);
    }
    if let Some(value) = line.strip_prefix("description:") {
      description = clean(value);
    }
  }
  (name, description)
}

fn strip_frontmatter(text: &str) -> &str {
  let Some(rest) = text.strip_prefix("---\n") else {
    return text;
  };
  let Some(end) = rest.find("\n---") else {
    return text;
  };
  rest[end + 4..].trim_start()
}

fn clean(value: &str) -> String {
  value.trim().trim_matches(['"', '\'']).to_string()
}
