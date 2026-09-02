// process-group control shared by the shell tool and background jobs. children are
// spawned into their own group so a timeout or cancel takes their descendants with them

use anyhow::{Context, Result, bail};

pub(crate) fn kill_group(pid: u32, signal: i32) -> Result<()> {
  let Ok(pid) = i32::try_from(pid) else {
    bail!("invalid process id {pid}");
  };
  // SAFETY: killpg takes two integers and has no memory-safety preconditions
  if unsafe { libc::killpg(pid, signal) } != 0 {
    return Err(std::io::Error::last_os_error()).with_context(|| format!("signal group {pid}"));
  }
  Ok(())
}

// kills the group on drop unless disarmed, so a dropped future cannot orphan grandchildren
pub(crate) struct GroupGuard(Option<u32>);

impl GroupGuard {
  pub(crate) fn new(pid: Option<u32>) -> Self {
    Self(pid)
  }

  pub(crate) fn disarm(mut self) {
    self.0 = None;
  }
}

impl Drop for GroupGuard {
  fn drop(&mut self) {
    if let Some(pid) = self.0 {
      drop(kill_group(pid, libc::SIGKILL));
    }
  }
}
