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

// on macOS one killpg does not empty a group whose shell is forking at that moment: the
// process being forked misses the signal and runs on with the group's id. the leader is dead
// after the first round and forks nothing more, so another round at once and a few more a
// few milliseconds apart catch most of what was in flight. a group that answers ESRCH is
// gone, and stopping there keeps a later group that reuses the id out of reach
pub(crate) fn kill_tree(pid: u32) {
  if kill_group(pid, libc::SIGKILL).is_err() || kill_group(pid, libc::SIGKILL).is_err() {
    return;
  }
  drop(
    std::thread::Builder::new()
      .name("killtree".into())
      .spawn(move || {
        for pause in [1, 4, 16, 64] {
          std::thread::sleep(std::time::Duration::from_millis(pause));
          if kill_group(pid, libc::SIGKILL).is_err() {
            return;
          }
        }
      }),
  );
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
      kill_tree(pid);
    }
  }
}
