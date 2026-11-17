//! Where scripts come from, the way Neovim finds them: an init.lua, then every `plugin/*.lua`
//! under the config directory and under each package in `pack/*/start/*`, whose `lua/` folders
//! `require` searches too. A project's own `.ainz/init.lua` runs only once it is trusted,
//! because it arrives with a checkout rather than from the person running it.

use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// The environment variable that names an init.lua, or `NONE` to run without any scripts.
pub const VARIABLE: &str = "AINZ_INIT";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
  pub init: Option<PathBuf>,
  /// Folders whose `plugin/` is sourced and whose `lua/` is on `package.path`, in order.
  pub roots: Vec<PathBuf>,
  pub project: Project,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Project {
  #[default]
  None,
  Trusted(PathBuf),
  /// There is one, and it has not been trusted in this form.
  Untrusted(PathBuf),
}

impl Sources {
  /// The files to run, in order: init.lua, each plugin file, then the project's.
  pub fn files(&self) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = self.init.iter().cloned().collect();
    for root in &self.roots {
      files.extend(plugins(root));
    }
    if let Project::Trusted(path) = &self.project {
      files.push(path.clone());
    }
    files
  }

  pub fn is_empty(&self) -> bool {
    self.files().is_empty()
  }
}

/// What there is to run for `workspace`, with `config` the directory config.toml lives in.
pub fn discover(config: &Path, workspace: &Path) -> Sources {
  let named = std::env::var_os(VARIABLE).map(PathBuf::from);
  if named.as_deref() == Some(Path::new("NONE")) {
    return Sources::default();
  }
  let init = named.or_else(|| candidates(config).into_iter().find(|path| path.is_file()));
  let home = init
    .as_deref()
    .and_then(Path::parent)
    .map(Path::to_path_buf)
    .unwrap_or_else(|| config.to_path_buf());
  let mut roots = vec![home.clone()];
  roots.extend(packages(&home));
  let project = workspace.join(".ainz/init.lua");
  let project = match project.is_file() {
    false => Project::None,
    true if trusted(config, &project) => Project::Trusted(project),
    true => Project::Untrusted(project),
  };
  Sources {
    init,
    roots,
    project,
  }
}

/// The config directory first, then the XDG one Neovim users will look in.
fn candidates(config: &Path) -> Vec<PathBuf> {
  let mut paths = vec![config.join("init.lua")];
  let xdg = std::env::var_os("XDG_CONFIG_HOME")
    .map(PathBuf::from)
    .or_else(|| dirs::home_dir().map(|home| home.join(".config")));
  if let Some(xdg) = xdg {
    let path = xdg.join("ainz/init.lua");
    if !paths.contains(&path) {
      paths.push(path);
    }
  }
  paths
}

fn packages(root: &Path) -> Vec<PathBuf> {
  let mut found = Vec::new();
  for pack in sorted(&root.join("pack")) {
    found.extend(
      sorted(&pack.join("start"))
        .into_iter()
        .filter(|path| path.is_dir()),
    );
  }
  found
}

fn plugins(root: &Path) -> Vec<PathBuf> {
  sorted(&root.join("plugin"))
    .into_iter()
    .filter(|path| path.extension().is_some_and(|extension| extension == "lua"))
    .collect()
}

fn sorted(dir: &Path) -> Vec<PathBuf> {
  let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
    .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
    .unwrap_or_default();
  paths.sort();
  paths
}

fn store(config: &Path) -> PathBuf {
  config.join("trust.json")
}

fn digest(path: &Path) -> Option<String> {
  let bytes = std::fs::read(path).ok()?;
  Some(format!("{:x}", Sha256::digest(bytes)))
}

fn entries(config: &Path) -> BTreeMap<String, String> {
  std::fs::read(store(config))
    .ok()
    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    .unwrap_or_default()
}

/// Trusted in exactly this form: an edit since makes it a stranger again.
pub fn trusted(config: &Path, path: &Path) -> bool {
  let Ok(path) = path.canonicalize() else {
    return false;
  };
  let known = entries(config);
  digest(&path).is_some_and(|digest| known.get(&*path.to_string_lossy()) == Some(&digest))
}

/// Records the file as it is now, so it runs from the next load on.
pub fn trust(config: &Path, path: &Path) -> Result<()> {
  let path = path
    .canonicalize()
    .with_context(|| format!("find {}", path.display()))?;
  let digest = digest(&path).with_context(|| format!("read {}", path.display()))?;
  let mut known = entries(config);
  known.insert(path.to_string_lossy().into_owned(), digest);
  std::fs::create_dir_all(config)?;
  let text = serde_json::to_vec_pretty(&known)?;
  let partial = store(config).with_extension("json.part");
  std::fs::write(&partial, text)?;
  std::fs::rename(&partial, store(config))?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
  }

  #[test]
  fn init_then_plugins_then_package_plugins_in_order() {
    let config = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let root = config.path();
    write(&root.join("init.lua"), "");
    write(&root.join("plugin/b.lua"), "");
    write(&root.join("plugin/a.lua"), "");
    write(&root.join("plugin/notes.txt"), "");
    write(&root.join("pack/mine/start/clock/plugin/clock.lua"), "");
    let sources = discover(root, workspace.path());
    let names: Vec<_> = sources
      .files()
      .iter()
      .map(|path| path.strip_prefix(root).unwrap().display().to_string())
      .collect();
    assert_eq!(
      names,
      [
        "init.lua",
        "plugin/a.lua",
        "plugin/b.lua",
        "pack/mine/start/clock/plugin/clock.lua"
      ]
    );
    assert_eq!(sources.roots.len(), 2);
  }

  #[test]
  fn a_project_script_runs_only_once_trusted_and_only_as_trusted() {
    let config = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let script = workspace.path().join(".ainz/init.lua");
    write(&script, "print('hi')");
    assert!(matches!(
      discover(config.path(), workspace.path()).project,
      Project::Untrusted(_)
    ));
    trust(config.path(), &script).unwrap();
    assert!(matches!(
      discover(config.path(), workspace.path()).project,
      Project::Trusted(_)
    ));
    write(&script, "os.execute('something else')");
    assert!(matches!(
      discover(config.path(), workspace.path()).project,
      Project::Untrusted(_)
    ));
  }
}
