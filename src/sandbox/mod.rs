//! An operating-system boundary around the commands Ainz runs for the model: the shell tool,
//! background jobs, hooks, plugin commands, and the checks improvement and evaluation run.
//!
//! It is off unless `AINZ_SANDBOX` asks for it. A shell confined to the workspace also cannot
//! fill `~/.cargo` or `~/.npm`, and quietly breaking every build people already run is worse
//! than the boundary is good. So the policy is stated, never guessed. When one is asked for and
//! cannot be provided, nothing runs: an unavailable sandbox is a refusal, never a silent fall
//! back to none.
//!
//! What it confines is writes, and under `strict` the network. Reads are left alone, because a
//! build reads half the machine and a boundary that has to be turned off to get work done is
//! not a boundary.

use std::{
  ffi::OsString,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

mod bubblewrap;
mod seatbelt;

/// macOS ships exactly one of these, at a fixed path. Nothing else is accepted: taking the name
/// off `PATH` would let anything earlier on it answer to `sandbox-exec` and be trusted to
/// confine the very command it is being asked to confine.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Bubblewrap, for the same reason at the same kind of fixed path.
pub const BWRAP: &str = "/usr/bin/bwrap";

/// The environment variable that states the policy.
pub const VARIABLE: &str = "AINZ_SANDBOX";

/// Ordered from least confined to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Policy {
  /// No OS boundary; the permission rules are the only gate.
  Off,
  /// Writes confined to the workspace, the temporary directories, and the usual devices.
  Workspace,
  /// That, and no network.
  Strict,
}

impl Policy {
  pub fn label(self) -> &'static str {
    match self {
      Self::Off => "off",
      Self::Workspace => "workspace",
      Self::Strict => "strict",
    }
  }
}

/// A value that is not a policy is an error rather than a shrug. Somebody who typed
/// `AINZ_SANDBOX=strcit` asked to be confined, and running them unconfined because of a typo is
/// the one outcome nobody wants.
pub fn parse(value: Option<&str>) -> Result<Policy> {
  let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
    return Ok(Policy::Off);
  };
  match value.to_ascii_lowercase().as_str() {
    "off" | "none" | "0" | "false" => Ok(Policy::Off),
    "workspace" | "on" | "1" | "true" => Ok(Policy::Workspace),
    "strict" => Ok(Policy::Strict),
    other => bail!("{VARIABLE}={other} is not a sandbox policy; use off, workspace, or strict"),
  }
}

/// What the environment asked for.
pub fn policy() -> Result<Policy> {
  parse(std::env::var(VARIABLE).ok().as_deref())
}

/// The same command, confined by the policy in force. The command's working directory is the
/// workspace that stays writable; `writable` names anything else outside it that must, such
/// as a job's log directory. Set stdio after this: it is not carried across.
pub fn confine(command: Command, writable: &[PathBuf]) -> Result<Command> {
  confine_with(policy()?, command, writable)
}

/// The same, with the policy given rather than read from the environment.
pub fn confine_with(policy: Policy, command: Command, writable: &[PathBuf]) -> Result<Command> {
  if policy == Policy::Off {
    return Ok(command);
  }
  let original = command.as_std();
  let workspace = original
    .get_current_dir()
    .context("a sandboxed command needs a working directory")?
    .to_path_buf();
  let mut argv: Vec<OsString> = vec![original.get_program().to_owned()];
  argv.extend(original.get_args().map(ToOwned::to_owned));
  let mut confined = build(policy, &workspace, &argv, writable)?;
  for (key, value) in original.get_envs() {
    match value {
      Some(value) => confined.env(key, value),
      None => confined.env_remove(key),
    };
  }
  Ok(confined)
}

/// `argv` under the confiner for `policy`, started in `workspace`.
pub fn build(
  policy: Policy,
  workspace: &Path,
  argv: &[OsString],
  writable: &[PathBuf],
) -> Result<Command> {
  let (program, args) = argv.split_first().context("no command to run")?;
  let mut process = match policy {
    Policy::Off => {
      let mut process = Command::new(program);
      process.args(args);
      process
    }
    Policy::Workspace | Policy::Strict => {
      let confiner = confiner(policy)?;
      let mut roots = vec![real(workspace), real(&std::env::temp_dir())];
      if cfg!(unix) {
        roots.push(real(Path::new("/tmp")));
      }
      roots.extend(writable.iter().map(|path| real(path)));
      roots.sort();
      roots.dedup();
      let mut process = Command::new(confiner);
      if confiner == SANDBOX_EXEC {
        // no `--`: sandbox-exec stops at the first argument that is not an option. Passing the
        // command as its own argv entries keeps it out of the profile, where it would be a
        // quoting bug waiting to happen
        process
          .arg("-p")
          .arg(seatbelt::profile(policy == Policy::Strict, &roots)?);
      } else {
        process.args(bubblewrap::args(
          policy == Policy::Strict,
          &roots,
          &real(workspace),
        ));
      }
      process.arg(program).args(args);
      process
    }
  };
  process.current_dir(workspace);
  Ok(process)
}

fn confiner(policy: Policy) -> Result<&'static str> {
  let confiner = if cfg!(target_os = "macos") {
    SANDBOX_EXEC
  } else if cfg!(target_os = "linux") {
    BWRAP
  } else {
    bail!(
      "{VARIABLE}={} was asked for, but Ainz can only sandbox on macOS and Linux; unset it to \
       run without one",
      policy.label()
    );
  };
  if !Path::new(confiner).exists() {
    bail!(
      "{VARIABLE}={} was asked for, but {confiner} is not on this machine; install it, or \
       unset {VARIABLE} to run without one",
      policy.label()
    );
  }
  Ok(confiner)
}

/// Seatbelt matches on real paths, so `/tmp` has to become `/private/tmp` before it is written
/// into the profile. A path that cannot be resolved is used as given: the profile is then
/// narrower than intended, never wider.
fn real(path: &Path) -> PathBuf {
  std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
#[path = "../../tests/sandbox/unit.rs"]
mod tests;
