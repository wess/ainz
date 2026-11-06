use anyhow::{Result, bail};
use tokio::process::Command;

use ainz::update::{self, Channel, FORMULA, State};

/// `ainz update`: the same work as the launch check, on demand and out loud.
pub async fn run(check: bool) -> Result<()> {
  let updater = update::local()?;
  let version = update::latest(&updater.base).await?;
  let state = State {
    checked_at: update::now(),
    latest: Some(version.clone()),
  };
  update::save(&updater.state, &state).await?;
  if !update::newer(&version, &updater.current) {
    println!("ainz {} is up to date", updater.current);
    return Ok(());
  }
  if check {
    println!("ainz {version} is available (running {})", updater.current);
    return Ok(());
  }
  match Channel::of(&updater.exe) {
    Channel::Standalone => {
      println!("updating ainz {} to {version}", updater.current);
      update::install(&updater.base, &version, &updater.exe).await?;
      println!("updated to ainz {version}; restart any running sessions to use it");
    }
    Channel::Homebrew => {
      let status = Command::new("brew")
        .args(["upgrade", FORMULA])
        .status()
        .await?;
      if !status.success() {
        bail!("brew upgrade {FORMULA} failed ({status})");
      }
    }
    Channel::Cargo => println!(
      "ainz {version} is available; this copy was built with cargo, so update it with:\n  \
       cargo install --git https://github.com/wess/ainz --tag v{version} --locked"
    ),
  }
  Ok(())
}
