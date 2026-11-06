//! Keeping an installed binary current. Only an install nothing else manages is replaced in
//! place: Homebrew and cargo own their files, so those are told what to run instead.

mod channel;
mod release;
mod state;
mod version;

use std::path::PathBuf;

use anyhow::{Context, Result};

pub use channel::{Channel, executable, unmarked};
pub use release::{archive, install, latest, target};
pub use state::{State, load, save};
pub use version::newer;

const RELEASES: &str = "https://github.com/wess/ainz/releases";
const INTERVAL: u64 = 24 * 60 * 60;
pub const FORMULA: &str = "wess/packages/ainz";

/// What one check works against, so a test can point it somewhere other than this machine.
pub struct Updater {
  pub current: String,
  pub base: String,
  pub state: PathBuf,
  pub exe: PathBuf,
}

pub fn local() -> Result<Updater> {
  let data = dirs::data_local_dir().context("could not locate the data directory")?;
  Ok(Updater {
    current: env!("CARGO_PKG_VERSION").into(),
    base: RELEASES.into(),
    state: data.join("ainz/update.json"),
    exe: executable()?.canonicalize()?,
  })
}

pub fn now() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map_or(0, |elapsed| elapsed.as_secs())
}

/// The launch-time check. Returns a line worth showing, or nothing when there is no news; being
/// offline is not news, and a check that fails waits for the next launch.
pub async fn background(updater: &Updater, now: u64) -> Option<String> {
  let mut known = load(&updater.state).await;
  let mut fresh = false;
  if now.saturating_sub(known.checked_at) >= INTERVAL
    && let Ok(version) = latest(&updater.base).await
  {
    known = State {
      checked_at: now,
      latest: Some(version),
    };
    fresh = true;
    drop(save(&updater.state, &known).await);
  }
  let version = known
    .latest
    .filter(|version| newer(version, &updater.current))?;
  match Channel::of(&updater.exe) {
    // a failed install is retried by the next check rather than on every launch
    Channel::Standalone if fresh => {
      Some(match install(&updater.base, &version, &updater.exe).await {
        Ok(()) => format!("updated to ainz {version}; restart to use it"),
        Err(error) => {
          format!("ainz {version} is available, but updating failed: {error:#}. Run ainz update")
        }
      })
    }
    _ => Some(format!("ainz {version} is available; run ainz update")),
  }
}
