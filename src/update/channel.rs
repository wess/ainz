use std::{
  ffi::OsStr,
  io,
  path::{Component, Path, PathBuf},
};

/// Who owns the installed file, which decides who may replace it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
  Homebrew,
  Cargo,
  Standalone,
}

impl Channel {
  pub fn of(exe: &Path) -> Self {
    let names: Vec<&OsStr> = exe
      .components()
      .filter_map(|part| match part {
        Component::Normal(name) => Some(name),
        _ => None,
      })
      .collect();
    let named = |name: &str| names.iter().any(|part| *part == name);
    if named("Cellar") {
      Self::Homebrew
    } else if named(".cargo") || named("target") {
      Self::Cargo
    } else {
      Self::Standalone
    }
  }
}

/// Linux reports a replaced executable as `path (deleted)`, and once an update lands that is
/// what a running session would try to start its workers from.
pub fn unmarked(path: PathBuf) -> PathBuf {
  match path
    .to_str()
    .and_then(|text| text.strip_suffix(" (deleted)"))
  {
    Some(text) => PathBuf::from(text),
    None => path,
  }
}

pub fn executable() -> io::Result<PathBuf> {
  std::env::current_exe().map(unmarked)
}
