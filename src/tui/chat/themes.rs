use super::*;
use ainz::theme::ThemeCatalog;

pub(super) async fn initialize(state: &mut ChatState, config: &Config) -> Result<()> {
  let catalog = ThemeCatalog::discover(&state.workspace).await?;
  match catalog.get(&config.ui.theme) {
    Some(theme) => state.theme = theme.clone(),
    None => state.primary.entries.push(Entry::new(
      EntryKind::Error,
      format!(
        "theme {} unavailable; using default. /themes shows errors",
        config.ui.theme
      ),
    )),
  }
  Ok(())
}

pub(super) async fn command(
  input: &str,
  state: &mut ChatState,
  config: &mut Config,
) -> Result<bool> {
  if input == "/theme" {
    state.primary.entries.push(Entry::new(
      EntryKind::System,
      format!("theme: {}", state.theme.name),
    ));
  } else if input == "/themes" {
    let catalog = ThemeCatalog::discover(&state.workspace).await?;
    let mut entries: Vec<_> = catalog
      .themes
      .iter()
      .map(|theme| {
        format!(
          "{}  {}",
          theme.name,
          if theme.path.as_os_str().is_empty() {
            "built-in".into()
          } else {
            theme.path.display().to_string()
          }
        )
      })
      .collect();
    entries.extend(
      catalog
        .issues
        .into_iter()
        .map(|issue| format!("invalid: {issue}")),
    );
    list_command(state, "themes", &entries)?;
  } else if let Some(name) = input.strip_prefix("/theme ") {
    let catalog = ThemeCatalog::discover(&state.workspace).await?;
    let theme = catalog
      .get(name.trim())
      .context("theme not found; use /themes to list files and errors")?;
    let previous = std::mem::replace(&mut config.ui.theme, theme.name.clone());
    if let Err(error) = config.save().await {
      config.ui.theme = previous;
      return Err(error);
    }
    state.theme = theme.clone();
    state.primary.entries.push(Entry::new(
      EntryKind::System,
      format!("theme {}; /theme default resets it", theme.name),
    ));
  } else {
    return Ok(false);
  }
  Ok(true)
}
