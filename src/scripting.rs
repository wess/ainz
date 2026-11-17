//! The session's scripts: loaded once at startup, before the configuration is used for
//! anything, and kept where the agent and the interface can both reach them.

use std::{
  path::{Path, PathBuf},
  sync::{Arc, PoisonError, RwLock},
};

use ainz::{
  Config,
  script::{Project, Script, files},
};

static SCRIPT: RwLock<Option<Arc<Script>>> = RwLock::new(None);
static NOTES: RwLock<Vec<String>> = RwLock::new(Vec::new());

pub fn current() -> Option<Arc<Script>> {
  SCRIPT
    .read()
    .unwrap_or_else(PoisonError::into_inner)
    .clone()
}

/// What loading said: errors from a script, or a project script waiting to be trusted. The
/// interface shows them once; a headless run prints them.
pub fn take_notes() -> Vec<String> {
  std::mem::take(&mut *NOTES.write().unwrap_or_else(PoisonError::into_inner))
}

pub fn config_dir() -> Option<PathBuf> {
  Config::path()
    .ok()
    .and_then(|path| path.parent().map(Path::to_path_buf))
}

/// Runs init.lua and the plugins beside it, which may change `config`. With nothing to run no
/// runtime is made at all; `clean` runs none of it, the way `nvim --clean` does.
pub fn start(workspace: &Path, config: &mut Config, clean: bool) {
  if clean {
    return;
  }
  let Some(dir) = config_dir() else {
    return;
  };
  let sources = files::discover(&dir, workspace);
  if sources.is_empty() && !matches!(sources.project, Project::Untrusted(_)) {
    return;
  }
  let notes = match Script::load(sources, workspace, config) {
    Ok((script, mut notes)) => {
      // what init.lua printed while it ran comes before what went wrong in it
      let printed: Vec<String> = script
        .take_notices()
        .into_iter()
        .map(|(text, _)| text)
        .collect();
      notes.splice(0..0, printed);
      *SCRIPT.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(script));
      notes
    }
    Err(error) => vec![format!("lua: {error:#}")],
  };
  NOTES
    .write()
    .unwrap_or_else(PoisonError::into_inner)
    .extend(notes);
}

/// `/source`: the scripts again, found afresh, so an init.lua written since startup or a
/// project script trusted a moment ago runs too.
pub fn reload(workspace: &Path, config: &mut Config) -> Vec<String> {
  let Some(dir) = config_dir() else {
    return vec!["no config directory to find scripts in".into()];
  };
  let sources = files::discover(&dir, workspace);
  if let Some(script) = current() {
    return script.reload(sources, config);
  }
  if sources.is_empty() {
    return vec![format!(
      "nothing to run; write {} to start",
      dir.join("init.lua").display()
    )];
  }
  match Script::load(sources, workspace, config) {
    Ok((script, notes)) => {
      *SCRIPT.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(script));
      notes
    }
    Err(error) => vec![format!("lua: {error:#}")],
  }
}

/// `/trust`: the workspace's own `.ainz/init.lua`, as it is now, may run.
pub fn trust(workspace: &Path) -> anyhow::Result<PathBuf> {
  let dir = config_dir().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
  let path = workspace.join(".ainz/init.lua");
  anyhow::ensure!(path.is_file(), "{} does not exist", path.display());
  files::trust(&dir, &path)?;
  Ok(path)
}
