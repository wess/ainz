//! The macOS boundary: a seatbelt profile for `sandbox-exec`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Later rules win in a seatbelt profile, so this reads as: allow everything, take writes away,
/// then hand back the writes that were the point of running the command.
pub(super) fn profile(strict: bool, writable: &[PathBuf]) -> Result<String> {
  let mut text = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
  text.push_str("(allow file-write*\n");
  for path in writable {
    text.push_str(&format!("  (subpath {})\n", quote(path)?));
  }
  text.push_str(DEVICES);
  text.push_str(")\n");
  if strict {
    text.push_str("(deny network*)\n");
  }
  Ok(text)
}

/// The devices a shell expects to write to, named one at a time rather than as `/dev`, which
/// would hand back the raw disks along with the terminal.
const DEVICES: &str = concat!(
  "  (literal \"/dev/null\")\n",
  "  (literal \"/dev/zero\")\n",
  "  (literal \"/dev/random\")\n",
  "  (literal \"/dev/urandom\")\n",
  "  (literal \"/dev/stdin\")\n",
  "  (literal \"/dev/stdout\")\n",
  "  (literal \"/dev/stderr\")\n",
  "  (literal \"/dev/tty\")\n",
  "  (literal \"/dev/dtracehelper\")\n",
  "  (regex #\"^/dev/fd/\")\n",
  "  (regex #\"^/dev/ttys[0-9]+$\")\n",
);

/// A path as a profile string literal. One that cannot be written as one, because it is not
/// UTF-8 or carries a control character that would end the expression early, is refused
/// rather than escaped and hoped for.
fn quote(path: &Path) -> Result<String> {
  let text = path.to_str().context("sandbox path is not UTF-8")?;
  if text.contains(char::is_control) {
    bail!(
      "sandbox path contains a control character: {}",
      path.display()
    );
  }
  Ok(format!(
    "\"{}\"",
    text.replace('\\', "\\\\").replace('"', "\\\"")
  ))
}
