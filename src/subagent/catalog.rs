use crate::{Config, config::PermissionMode, frontmatter};
use anyhow::{Context, Result};
use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};
use tokio::fs;
#[derive(Clone, Debug)]
pub struct AgentDefinition {
  pub name: String,
  pub description: String,
  pub instructions: String,
  pub tools: Option<Vec<String>>,
  /// The model the definition asks for, on the session's configured provider.
  pub model: Option<String>,
  pub permission_mode: Option<PermissionMode>,
  pub disallowed_tools: Option<Vec<String>>,
  pub max_steps: Option<usize>,
}

impl AgentDefinition {
  /// What this type narrows, in the order a reader cares about. Empty when the type is the
  /// session with different instructions, which is the common case.
  pub fn constraints(&self) -> Vec<String> {
    let mut constraints = Vec::new();
    if let Some(tools) = &self.tools {
      constraints.push(format!("tools: {}", tools.join(", ")));
    }
    if let Some(model) = &self.model {
      constraints.push(format!("asks for model {model}"));
    }
    if let Some(mode) = self.permission_mode {
      constraints.push(format!(
        "permission: {}",
        match mode {
          PermissionMode::Ask => "ask",
          PermissionMode::Auto => "auto",
          PermissionMode::ReadOnly => "read-only",
        }
      ));
    }
    if let Some(tools) = &self.disallowed_tools {
      constraints.push(format!("without: {}", tools.join(", ")));
    }
    if let Some(steps) = self.max_steps {
      constraints.push(format!("max turns: {steps}"));
    }
    constraints
  }

  /// The one-line form the tool description lists a type by.
  pub fn summary(&self) -> String {
    let mut detail = self.constraints();
    if !self.description.is_empty() {
      detail.insert(0, self.description.clone());
    }
    if detail.is_empty() {
      self.name.clone()
    } else {
      format!("{} ({})", self.name, detail.join("; "))
    }
  }
}

#[derive(Clone, Debug, Default)]
pub struct AgentCatalog {
  pub agents: Vec<AgentDefinition>,
}

impl AgentCatalog {
  pub async fn discover(workspace: &Path) -> Result<Self> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
      roots.push(home.join(".claude/agents"));
    }
    if let Some(config) = Config::path()
      .ok()
      .and_then(|path| path.parent().map(Path::to_path_buf))
    {
      roots.push(config.join("agents"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    for path in ancestors {
      roots.push(path.join(".claude/agents"));
      roots.push(path.join(".ainz/agents"));
    }

    let mut agents = BTreeMap::new();
    for root in roots {
      let mut entries = match fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
        Err(error) => return Err(error).with_context(|| format!("read {}", root.display())),
      };
      while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "md") {
          continue;
        }
        let Ok(text) = fs::read_to_string(&path).await else {
          continue;
        };
        let front = frontmatter::parse(&text);
        let fallback = path
          .file_stem()
          .unwrap_or_default()
          .to_string_lossy()
          .into_owned();
        let name = front.field("name").unwrap_or(fallback);
        if name.trim().is_empty() {
          continue;
        }
        // the same key is spelled three ways across the harnesses these files come from
        let field = |keys: [&str; 3]| keys.into_iter().find_map(|key| front.field(key));
        let permission_mode = field(["permissionMode", "permission-mode", "permission_mode"])
          .and_then(|mode| match mode.as_str() {
            "read_only" | "read-only" | "plan" => Some(PermissionMode::ReadOnly),
            "ask" | "default" => Some(PermissionMode::Ask),
            "auto" => Some(PermissionMode::Auto),
            _ => None,
          });
        let max_steps = field(["maxTurns", "max-turns", "max_steps"])
          .and_then(|steps| steps.parse().ok())
          .filter(|steps| *steps > 0);
        agents.insert(
          name.clone(),
          AgentDefinition {
            name,
            description: front.field("description").unwrap_or_default(),
            instructions: front.body.trim().to_string(),
            tools: front.field("tools").and_then(tool_list),
            model: front.field("model").filter(|model| model != "inherit"),
            permission_mode,
            disallowed_tools: field(["disallowedTools", "disallowed-tools", "disallowed_tools"])
              .and_then(tool_list),
            max_steps,
          },
        );
      }
    }
    Ok(Self {
      agents: agents.into_values().collect(),
    })
  }

  pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
    self.agents.iter().find(|agent| agent.name == name)
  }
}

/// A comma-separated list of tool names, where `*` and an empty list both mean no narrowing.
fn tool_list(value: String) -> Option<Vec<String>> {
  let tools: Vec<_> = value
    .split(',')
    .map(|tool| match tool.trim().to_ascii_lowercase().as_str() {
      "bash" => "shell".into(),
      "grep" => "search".into(),
      "ls" => "list".into(),
      "webfetch" => "fetch".into(),
      value => value.to_owned(),
    })
    .filter(|tool| !tool.is_empty() && tool != "*")
    .collect();
  (!tools.is_empty()).then_some(tools)
}
