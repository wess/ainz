use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tokio::fs;

use crate::frontmatter;

#[derive(Clone, Debug)]
pub struct PromptTemplate {
  pub name: String,
  pub description: String,
  // what the arguments are for, shown in the palette instead of a bare [ARGS]
  pub hint: Option<String>,
  pub path: PathBuf,
}

const MAX_DEPTH: usize = 4;

#[derive(Clone, Debug, Default)]
pub struct PromptCatalog {
  pub prompts: Vec<PromptTemplate>,
  // templates that could not be read, reported beside the rest rather than costing them
  pub issues: Vec<String>,
}

impl PromptCatalog {
  pub async fn discover(workspace: &Path) -> Result<Self> {
    // later roots win, so a project's own template replaces a shared or user one
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
      roots.push(home.join(".claude/commands"));
    }
    if let Some(config) = dirs::config_dir() {
      roots.push(config.join("agentx/prompts"));
      roots.push(config.join("ainz/prompts"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    for path in ancestors {
      roots.push(path.join(".claude/commands"));
      roots.push(path.join(".agentx/prompts"));
      roots.push(path.join(".ainz/prompts"));
    }

    let mut prompts = BTreeMap::new();
    let mut issues = Vec::new();
    for root in roots {
      for prompt in discover_root(&root, &mut issues).await {
        prompts.insert(prompt.name.clone(), prompt);
      }
    }
    Ok(Self {
      prompts: prompts.into_values().collect(),
      issues,
    })
  }

  pub async fn expand(&self, name: &str, args: &[String]) -> Result<String> {
    let prompt = self
      .prompts
      .iter()
      .find(|prompt| prompt.name == name)
      .with_context(|| format!("prompt template {name} was not found"))?;
    let text = fs::read_to_string(&prompt.path).await?;
    let joined = args.join(" ");
    let mut output = frontmatter::parse(&text)
      .body
      .replace("{{args}}", &joined)
      .replace("$ARGUMENTS", &joined);
    // highest index first, so $1 cannot eat the leading digit of $10
    for (index, value) in args.iter().enumerate().rev() {
      output = output.replace(&format!("{{{{{}}}}}", index + 1), value);
      output = output.replace(&format!("${}", index + 1), value);
    }
    Ok(output)
  }
}

// subdirectories namespace their templates, so commands/review/api.md becomes /review:api.
// the directories are shared with other tools, so one file that cannot be read costs only
// itself and is reported, not every other command with it
async fn discover_root(root: &Path, issues: &mut Vec<String>) -> Vec<PromptTemplate> {
  let mut prompts = Vec::new();
  let mut pending = vec![(root.to_path_buf(), String::new(), 0_usize)];
  while let Some((directory, prefix, depth)) = pending.pop() {
    let mut entries = match fs::read_dir(&directory).await {
      Ok(entries) => entries,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => {
        issues.push(format!("{}: {error}", directory.display()));
        continue;
      }
    };
    loop {
      let entry = match entries.next_entry().await {
        Ok(Some(entry)) => entry,
        Ok(None) => break,
        Err(error) => {
          issues.push(format!("{}: {error}", directory.display()));
          break;
        }
      };
      let segment = entry.file_name().to_string_lossy().into_owned();
      if segment.starts_with('.') {
        continue;
      }
      let path = entry.path();
      // metadata follows a symlink, so a linked folder of commands still counts
      if fs::metadata(&path).await.is_ok_and(|meta| meta.is_dir()) {
        if depth + 1 < MAX_DEPTH {
          pending.push((path, format!("{prefix}{segment}:"), depth + 1));
        }
        continue;
      }
      if path.extension().is_none_or(|extension| extension != "md") {
        continue;
      }
      let text = match fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(error) => {
          issues.push(format!("{}: {error}", path.display()));
          continue;
        }
      };
      let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
      let front = frontmatter::parse(&text);
      let name = front
        .field("name")
        .unwrap_or_else(|| format!("{prefix}{stem}"));
      if !name.is_empty() {
        prompts.push(PromptTemplate {
          name,
          description: front.field("description").unwrap_or_default(),
          hint: front.field("argument-hint"),
          path,
        });
      }
    }
  }
  prompts
}
