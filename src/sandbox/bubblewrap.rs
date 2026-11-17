//! The Linux boundary: bubblewrap namespaces. The whole filesystem is mounted read-only, the
//! writable roots are mounted back over it read-write, `/dev` is a fresh one holding only the
//! ordinary devices, and under `strict` the network namespace has nothing in it but loopback.

use std::{
  ffi::OsString,
  path::{Path, PathBuf},
};

pub(super) fn args(strict: bool, writable: &[PathBuf], workspace: &Path) -> Vec<OsString> {
  let mut args: Vec<OsString> = ["--die-with-parent", "--unshare-pid"]
    .into_iter()
    .map(Into::into)
    .collect();
  if strict {
    args.push("--unshare-net".into());
  }
  for arg in ["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"] {
    args.push(arg.into());
  }
  for path in writable {
    args.extend(["--bind".into(), path.into(), path.into()]);
  }
  args.extend(["--chdir".into(), workspace.into(), "--".into()]);
  args
}
