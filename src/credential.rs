use std::{env, fmt};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Where a provider's API key comes from. Ainz never stores a secret itself — only a pointer to
/// where one lives — so `resolve` re-reads the source each time rather than caching a value.
#[derive(Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum Credential {
  #[default]
  None,
  Env {
    var: String,
  },
  /// a secret kept in Synapse's vault (e.g. "apis.OpenRouter"), exported to `var` when resolved
  Synapse {
    secret: String,
    var: String,
  },
  /// a token the user typed, kept in the OS keychain under this account name, never in the config
  Keychain {
    account: String,
  },
}

// derived Debug would be fine here too — none of these fields ever hold a value, only names and
// pointers to where one lives — but a manual impl keeps that guarantee even if a value-bearing
// field is added later without anyone updating this.
impl fmt::Debug for Credential {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::None => write!(f, "None"),
      Self::Env { var } => f.debug_struct("Env").field("var", var).finish(),
      Self::Synapse { secret, var } => f
        .debug_struct("Synapse")
        .field("secret", secret)
        .field("var", var)
        .finish(),
      Self::Keychain { account } => f
        .debug_struct("Keychain")
        .field("account", account)
        .finish(),
    }
  }
}

impl Credential {
  pub async fn resolve(&self) -> Result<Option<String>> {
    match self {
      Self::None => Ok(None),
      Self::Env { var } => Ok(read_env(var)),
      Self::Synapse { var, .. } => resolve_synapse(var).await,
      Self::Keychain { account } => resolve_keychain(account).await,
    }
  }
}

fn read_env(var: &str) -> Option<String> {
  match env::var(var) {
    Ok(value) if !value.trim().is_empty() => Some(value),
    // unset, empty, or not valid unicode: all read the same as "nothing configured here"
    _ => None,
  }
}

fn non_empty(bytes: &[u8]) -> Option<String> {
  let value = String::from_utf8_lossy(bytes).trim().to_string();
  (!value.is_empty()).then_some(value)
}

/// a secret scoped to another directory's vault is a legitimate miss, not a failure — so every
/// exit here is `Ok(None)`, never `Err`
async fn resolve_synapse(var: &str) -> Result<Option<String>> {
  let output = match Command::new("synapse")
    .args(["run", "--", "printenv", var])
    .output()
    .await
  {
    Ok(output) => output,
    Err(_) => return Ok(None),
  };
  if !output.status.success() {
    return Ok(None);
  }
  Ok(non_empty(&output.stdout))
}

async fn resolve_keychain(account: &str) -> Result<Option<String>> {
  #[cfg(target_os = "macos")]
  {
    let output = match Command::new("security")
      .args(["find-generic-password", "-s", "ainz", "-a", account, "-w"])
      .output()
      .await
    {
      Ok(output) => output,
      Err(_) => return Ok(None),
    };
    if !output.status.success() {
      return Ok(None);
    }
    Ok(non_empty(&output.stdout))
  }
  #[cfg(target_os = "linux")]
  {
    let output = match Command::new("secret-tool")
      .args(["lookup", "service", "ainz", "account", account])
      .output()
      .await
    {
      Ok(output) => output,
      Err(_) => return Ok(None),
    };
    if !output.status.success() {
      return Ok(None);
    }
    Ok(non_empty(&output.stdout))
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  {
    let _ = account;
    Ok(None)
  }
}

/// Write a typed token to the keychain. macOS `security -w` takes the value as an argument —
/// the tool offers no stdin form, so it briefly appears in the process list of `ps` — while the
/// Linux `secret-tool store` command reads it from stdin, so it never does there.
pub async fn store(account: &str, value: &str) -> Result<()> {
  #[cfg(target_os = "macos")]
  {
    let status = Command::new("security")
      .args([
        "add-generic-password",
        "-U",
        "-s",
        "ainz",
        "-a",
        account,
        "-w",
        value,
      ])
      .status()
      .await
      .context("run security add-generic-password")?;
    if status.success() {
      Ok(())
    } else {
      bail!("security add-generic-password failed")
    }
  }
  #[cfg(target_os = "linux")]
  {
    use tokio::io::AsyncWriteExt;

    let mut child = Command::new("secret-tool")
      .args([
        "store",
        "--label=ainz",
        "service",
        "ainz",
        "account",
        account,
      ])
      .stdin(std::process::Stdio::piped())
      .spawn()
      .context("run secret-tool store")?;
    let mut stdin = child.stdin.take().context("secret-tool gave no stdin")?;
    stdin
      .write_all(value.as_bytes())
      .await
      .context("write to secret-tool")?;
    drop(stdin);
    let status = child.wait().await.context("wait for secret-tool")?;
    if status.success() {
      Ok(())
    } else {
      bail!("secret-tool store failed")
    }
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  {
    let _ = (account, value);
    bail!(
      "no OS keychain on this platform; use an environment variable or a Synapse secret instead"
    )
  }
}

/// Whether `store` can work here, so setup can avoid offering what will fail.
pub fn keychain_available() -> bool {
  #[cfg(target_os = "macos")]
  {
    true
  }
  #[cfg(target_os = "linux")]
  {
    on_path("secret-tool")
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  {
    false
  }
}

#[cfg(target_os = "linux")]
fn on_path(command: &str) -> bool {
  env::var_os("PATH")
    .map(|path| env::split_paths(&path).any(|dir| dir.join(command).is_file()))
    .unwrap_or(false)
}
