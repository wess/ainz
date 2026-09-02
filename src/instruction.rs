use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const BASE: &str = "You are a coding agent working in the supplied workspace. Be concise. Inspect before changing files. Use tools to verify work. Never claim a command succeeded unless its output proves it.";

pub async fn load(workspace: &Path) -> Result<String> {
  let mut files = Vec::new();
  if let Some(config) = dirs::config_dir() {
    let current = config.join("agentx/AGENTS.md");
    if current.exists() {
      files.push(current);
    } else {
      files.push(config.join("struts/AGENTS.md"));
    }
  }

  let mut ancestors: Vec<PathBuf> = workspace.ancestors().map(Path::to_path_buf).collect();
  ancestors.reverse();
  files.extend(ancestors.into_iter().map(|path| path.join("AGENTS.md")));

  let mut sections = vec![BASE.to_string()];
  for path in files {
    match tokio::fs::read_to_string(&path).await {
      Ok(text) if !text.trim().is_empty() => {
        sections.push(format!(
          "Instructions from {}:\n{}",
          path.display(),
          text.trim()
        ));
      }
      Ok(_) => {}
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
      Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    }
  }

  Ok(sections.join("\n\n"))
}
