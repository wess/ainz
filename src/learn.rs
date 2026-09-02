use std::{
  path::{Path, PathBuf},
  sync::Arc,
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::fs;

use crate::{
  frontmatter,
  protocol::ToolSpec,
  synapse::Synapse,
  tool::{Risk, Tool, ToolContext, truncate},
};

/// Where a session writes a procedure it worked out. The gate is on installing rather than on
/// writing: a taught skill sits in a proposal list and reaches no session until it is approved.
#[derive(Clone)]
pub enum Teacher {
  Local(LocalSkills),
  Synapse(Synapse),
}

impl Teacher {
  pub fn local() -> Result<Self> {
    Ok(Self::Local(LocalSkills::new()?))
  }

  pub fn tool(&self) -> Arc<dyn Tool> {
    Arc::new(LearnTool {
      teacher: self.clone(),
    })
  }

  pub async fn teach(
    &self,
    name: &str,
    description: &str,
    instructions: &str,
    scope: &str,
    note: Option<&str>,
  ) -> Result<String> {
    check_name(name)?;
    if description.trim().is_empty() || instructions.trim().is_empty() {
      bail!("a skill needs both a one-line description and its instructions");
    }
    match self {
      Self::Local(local) => local.teach(name, description, instructions, note).await,
      Self::Synapse(synapse) => {
        synapse
          .teach(name, description, instructions, scope, note)
          .await
      }
    }
  }

  pub async fn revise(
    &self,
    name: &str,
    instructions: &str,
    description: Option<&str>,
    note: Option<&str>,
  ) -> Result<String> {
    check_name(name)?;
    if instructions.trim().is_empty() {
      bail!("a correction replaces the instructions, so it cannot be empty");
    }
    match self {
      Self::Local(local) => local.revise(name, instructions, description, note).await,
      Self::Synapse(synapse) => synapse.revise(name, instructions, description, note).await,
    }
  }
}

#[derive(Clone, Debug)]
pub struct ProposedSkill {
  pub name: String,
  pub description: String,
  pub path: PathBuf,
}

/// `SKILL.md` files under the Ainz config directory. Proposals sit in `skills/proposed/`, which
/// skill discovery does not read, so writing one costs a line in a list rather than context in
/// every session.
#[derive(Clone, Debug)]
pub struct LocalSkills {
  root: PathBuf,
}

impl LocalSkills {
  pub fn new() -> Result<Self> {
    let base = dirs::config_dir().context("could not locate the config directory")?;
    Ok(Self::with_root(base.join("ainz/skills")))
  }

  pub fn with_root(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn proposed_dir(&self) -> PathBuf {
    self.root.join("proposed")
  }

  fn installed(&self, name: &str) -> PathBuf {
    self.root.join(name).join("SKILL.md")
  }

  fn proposal(&self, name: &str) -> PathBuf {
    self.proposed_dir().join(name).join("SKILL.md")
  }

  async fn teach(
    &self,
    name: &str,
    description: &str,
    instructions: &str,
    note: Option<&str>,
  ) -> Result<String> {
    if self.installed(name).exists() {
      bail!("skill {name} already exists; correct it with revise instead of teaching it again");
    }
    let path = self.proposal(name);
    write_skill(&path, name, description, instructions, note).await?;
    Ok(format!(
      "proposed skill {name}; it reaches no session until `ainz skills approve {name}`"
    ))
  }

  // a correction reaches the installed copy: a session running the version that was wrong is
  // the thing being fixed. the replaced text is kept beside it.
  async fn revise(
    &self,
    name: &str,
    instructions: &str,
    description: Option<&str>,
    note: Option<&str>,
  ) -> Result<String> {
    let installed = self.installed(name);
    let (path, existing) = match fs::read_to_string(&installed).await {
      Ok(text) => (installed, Some(text)),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
        let proposal = self.proposal(name);
        let text = fs::read_to_string(&proposal)
          .await
          .with_context(|| format!("skill {name} was not found in {}", self.root.display()))?;
        (proposal, Some(text))
      }
      Err(error) => return Err(error).with_context(|| format!("read {}", installed.display())),
    };
    let previous = existing.unwrap_or_default();
    let front = frontmatter::parse(&previous);
    let description = description
      .map(str::to_string)
      .or_else(|| front.field("description"))
      .unwrap_or_default();
    let backup = path.with_file_name(format!("SKILL.{}.md", now()));
    fs::write(&backup, &previous)
      .await
      .with_context(|| format!("write {}", backup.display()))?;
    write_skill(&path, name, &description, instructions, note).await?;
    Ok(format!(
      "revised {name}; the replaced version is at {}",
      backup.display()
    ))
  }

  pub async fn proposed(&self) -> Result<Vec<ProposedSkill>> {
    let directory = self.proposed_dir();
    let mut entries = match fs::read_dir(&directory).await {
      Ok(entries) => entries,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
      Err(error) => return Err(error).with_context(|| format!("read {}", directory.display())),
    };
    let mut skills = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
      let path = entry.path().join("SKILL.md");
      let Ok(text) = fs::read_to_string(&path).await else {
        continue;
      };
      let front = frontmatter::parse(&text);
      let name = front.field("name").unwrap_or_else(|| {
        entry
          .file_name()
          .to_string_lossy()
          .trim_end_matches('/')
          .to_string()
      });
      skills.push(ProposedSkill {
        name,
        description: front.field("description").unwrap_or_default(),
        path,
      });
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
  }

  pub async fn approve(&self, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let source = self.proposed_dir().join(name);
    if !source.join("SKILL.md").exists() {
      bail!("no proposed skill named {name}");
    }
    let destination = self.root.join(name);
    if destination.exists() {
      bail!("skill {name} already exists at {}", destination.display());
    }
    fs::rename(&source, &destination)
      .await
      .with_context(|| format!("install {}", destination.display()))?;
    Ok(destination)
  }

  pub async fn reject(&self, name: &str) -> Result<()> {
    check_name(name)?;
    let source = self.proposed_dir().join(name);
    if !source.exists() {
      bail!("no proposed skill named {name}");
    }
    fs::remove_dir_all(&source)
      .await
      .with_context(|| format!("remove {}", source.display()))
  }
}

async fn write_skill(
  path: &Path,
  name: &str,
  description: &str,
  instructions: &str,
  note: Option<&str>,
) -> Result<()> {
  let directory = path.parent().context("skill path has no directory")?;
  fs::create_dir_all(directory)
    .await
    .with_context(|| format!("create {}", directory.display()))?;
  let mut text = format!("---\nname: {name}\ndescription: {}\n", description.trim());
  if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
    text.push_str(&format!("note: {}\n", note.trim()));
  }
  text.push_str("---\n\n");
  text.push_str(instructions.trim());
  text.push('\n');
  fs::write(path, text)
    .await
    .with_context(|| format!("write {}", path.display()))
}

pub fn check_name(name: &str) -> Result<()> {
  let valid = !name.is_empty()
    && name.len() <= 64
    && name
      .chars()
      .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    && !name.starts_with('-');
  if !valid {
    bail!("skill name {name:?} must be lowercase words joined by hyphens");
  }
  Ok(())
}

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}

struct LearnTool {
  teacher: Teacher,
}

#[derive(Deserialize)]
struct LearnArgs {
  action: String,
  name: String,
  description: Option<String>,
  instructions: String,
  scope: Option<String>,
  note: Option<String>,
}

#[async_trait]
impl Tool for LearnTool {
  fn spec(&self) -> ToolSpec {
    let gate = match self.teacher {
      Teacher::Local(_) => "A taught skill waits for approval before any session loads it.",
      Teacher::Synapse(_) => {
        "Synapse holds a taught skill until the user approves it; a correction reaches the \
         installed copies immediately."
      }
    };
    ToolSpec {
      name: "learn".into(),
      description: format!(
        "Write down a procedure this session worked out so a later one can follow it, as a \
         skill. Use `teach` after finishing something you had to figure out — a sequence of \
         steps, a gotcha and its fix, a release or debugging routine. Use `revise` when a \
         skill you loaded turned out wrong or out of date. Do not record one-off answers or \
         what the repository already documents. {gate}"
      ),
      parameters: json!({
        "type": "object", "properties": {
          "action": {"type": "string", "enum": ["teach", "revise"]},
          "name": {"type": "string"},
          "description": {"type": "string"},
          "instructions": {"type": "string"},
          "scope": {"type": "string", "enum": ["project", "global"]},
          "note": {"type": "string"}
        }, "required": ["action", "name", "instructions"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Write
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let args: LearnArgs = serde_json::from_value(arguments)?;
    let output = match args.action.as_str() {
      "teach" => {
        let description = args
          .description
          .context("description is required to teach a skill")?;
        self
          .teacher
          .teach(
            &args.name,
            &description,
            &args.instructions,
            args.scope.as_deref().unwrap_or("project"),
            args.note.as_deref(),
          )
          .await?
      }
      "revise" => {
        self
          .teacher
          .revise(
            &args.name,
            &args.instructions,
            args.description.as_deref(),
            args.note.as_deref(),
          )
          .await?
      }
      other => bail!("unknown learn action: {other}"),
    };
    Ok(truncate(output, context.max_output_bytes))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn names_are_lowercase_and_hyphenated() {
    assert!(check_name("cut-a-release").is_ok());
    assert!(check_name("Cut A Release").is_err());
    assert!(check_name("-leading").is_err());
    assert!(check_name("").is_err());
  }
}
