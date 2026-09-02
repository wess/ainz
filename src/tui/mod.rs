use std::{
  io::{self, Stdout},
  sync::{
    Once,
    atomic::{AtomicBool, Ordering},
  },
};

use ainz::{Config, HttpProvider, ProcessOutput, ProviderConfig, ProviderKind};
use anyhow::{Context, Result};
use crossterm::{
  event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as InputEvent, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
  },
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
  Frame, Terminal, TerminalOptions, Viewport,
  backend::CrosstermBackend,
  layout::{Alignment, Constraint, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::command::{ProviderPreset, preset_profile};

mod chat;
mod import;
mod input;
mod masthead;
mod settings;

pub(crate) use chat::{ChatNext, run_chat};
pub(crate) use import::import;
pub(crate) use settings::settings;

pub(super) type Term = Terminal<CrosstermBackend<Stdout>>;

const INK: Color = Color::Rgb(218, 222, 226);
const MUTED: Color = Color::Rgb(128, 138, 148);
const ACCENT: Color = Color::Rgb(83, 196, 190);
const ACTIVE: Color = Color::Rgb(145, 210, 138);
const BLUE: Color = Color::Rgb(24, 66, 128);
const CYAN: Color = Color::Rgb(72, 205, 214);
const YELLOW: Color = Color::Rgb(230, 199, 92);
const RED: Color = Color::Rgb(224, 103, 103);
const MAGENTA: Color = Color::Rgb(198, 118, 205);

#[derive(Clone)]
enum Choice {
  Preset(ProviderPreset),
  Existing(String),
  Http,
  Process,
}

impl Choice {
  fn name(&self) -> &str {
    match self {
      Self::Preset(ProviderPreset::Ollama) => "Ollama",
      Self::Preset(ProviderPreset::LiteLlm) => "LiteLLM",
      Self::Preset(ProviderPreset::Codex) => "Codex CLI",
      Self::Preset(ProviderPreset::ClaudeCode) => "Claude Code",
      Self::Existing(name) => name,
      Self::Http => "Custom HTTP",
      Self::Process => "Custom process",
    }
  }

  fn detail(&self) -> (&str, &str) {
    match self {
      Self::Preset(ProviderPreset::Ollama) => (
        "Local models",
        "Connects to the local server and discovers installed models automatically.",
      ),
      Self::Preset(ProviderPreset::LiteLlm) => (
        "Proxy for every other model",
        concat!(
          "Points at a LiteLLM proxy and lists the models it serves. ",
          "The key stays in an environment variable, so one endpoint covers every provider ",
          "behind it."
        ),
      ),
      Self::Preset(ProviderPreset::Codex) => (
        "Headless coding agent",
        concat!(
          "Runs the installed CLI noninteractively. Read-only by default; ",
          "workspace writes follow automatic permission mode."
        ),
      ),
      Self::Preset(ProviderPreset::ClaudeCode) => (
        "Headless coding agent",
        concat!(
          "Runs the installed CLI in print mode and shows its answer and tool calls as they ",
          "happen. The CLI keeps authority over its own tools."
        ),
      ),
      Self::Existing(_) => (
        "Saved provider",
        "Switch to this profile, then choose one of its known models or enter another.",
      ),
      Self::Http => (
        "Compatible endpoint",
        "Add a chat-completions endpoint. Credentials stay in an environment variable.",
      ),
      Self::Process => (
        "Executable adapter",
        concat!(
          "Send the transcript over stdin and read the reply from stdout: plain text, a JSON ",
          "result field, or a stream of JSON lines reported as they are written."
        ),
      ),
    }
  }

  fn key(&self) -> Option<&str> {
    match self {
      Self::Preset(ProviderPreset::Ollama) => Some("ollama"),
      Self::Preset(ProviderPreset::LiteLlm) => Some("litellm"),
      Self::Preset(ProviderPreset::Codex) => Some("codex"),
      Self::Preset(ProviderPreset::ClaudeCode) => Some("claude"),
      Self::Existing(name) => Some(name),
      Self::Http | Self::Process => None,
    }
  }
}

async fn discover_models(
  config: &Config,
  provider: &ProviderConfig,
  endpoint: String,
) -> Result<Vec<String>> {
  let key = config.api_key_for(provider)?;
  HttpProvider::new(endpoint, String::new(), key)?
    .models()
    .await
}

/// What the tool's own config says it is set to, so its model is offered rather than recalled.
fn configured_model(relative: &str, key: &str) -> Option<String> {
  let path = dirs::home_dir()?.join(relative);
  let text = std::fs::read_to_string(path).ok()?;
  match path_is_json(relative) {
    true => serde_json::from_str::<serde_json::Value>(&text)
      .ok()?
      .get(key)?
      .as_str()
      .map(str::to_string),
    false => text.lines().find_map(|line| {
      let (name, value) = line.split_once('=')?;
      (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
    }),
  }
  .filter(|model| !model.is_empty())
}

fn path_is_json(path: &str) -> bool {
  path.ends_with(".json")
}

/// A list to pick from, ending in a row that falls back to typing the value by hand.
fn choose_value(
  terminal: &mut Term,
  title: &str,
  subtitle: &str,
  label: &'static str,
  items: Vec<String>,
  typed: &str,
) -> Result<Option<String>> {
  if items.is_empty() {
    return Ok(
      edit_fields(terminal, title, vec![Field::new(label, typed)])?.map(|values| values[0].clone()),
    );
  }
  let mut rows = items.clone();
  rows.push("Type another…".into());
  let mut selected = 0;
  let mut list = Rect::default();
  loop {
    terminal.draw(|frame| {
      let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(1),
      ])
      .areas(frame.area());
      render_header(frame, header, title, subtitle);
      let area = centered(body, body.width.min(82), body.height);
      list = area;
      let mut state = ListState::default().with_selected(Some(selected));
      frame.render_stateful_widget(
        List::new(rows.iter().map(|row| ListItem::new(row.clone())))
          .block(
            Block::default()
              .title(format!(" {label} "))
              .borders(Borders::ALL)
              .border_style(Style::default().fg(MUTED))
              .padding(ratatui::widgets::Padding::horizontal(1)),
          )
          .highlight_style(
            Style::default()
              .fg(Color::Black)
              .bg(ACCENT)
              .add_modifier(Modifier::BOLD),
          ),
        area,
        &mut state,
      );
      render_footer(frame, footer, "↑↓ choose   enter select   esc back");
    })?;
    match event::read()? {
      InputEvent::Mouse(mouse) => match mouse.kind {
        MouseEventKind::ScrollUp => selected = selected.saturating_sub(1),
        MouseEventKind::ScrollDown => selected = (selected + 1).min(rows.len() - 1),
        // the row under the pointer, past the box's own border line
        MouseEventKind::Down(_) => {
          if let Some(index) = mouse.row.checked_sub(list.y + 1).map(usize::from)
            && index < rows.len()
          {
            selected = index;
          }
        }
        _ => {}
      },
      InputEvent::Key(key) if key.kind != KeyEventKind::Release => match key.code {
        KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(rows.len() - 1),
        // typing another value starts from nothing: the default is already a row above
        KeyCode::Enter if selected == rows.len() - 1 => {
          return Ok(
            edit_fields(terminal, title, vec![Field::new(label, "")])?
              .map(|values| values[0].clone()),
          );
        }
        KeyCode::Enter => return Ok(Some(rows[selected].clone())),
        KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
        _ => {}
      },
      _ => {}
    }
  }
}

/// Environment variables that look like credentials, so a key never has to be typed from memory.
fn credential_variables() -> Vec<String> {
  let mut found: Vec<String> = std::env::vars()
    .filter(|(name, value)| {
      !value.trim().is_empty()
        && (name.ends_with("_API_KEY") || name.ends_with("_TOKEN") || name.ends_with("_KEY"))
    })
    .map(|(name, _)| name)
    .collect();
  found.sort();
  found.dedup();
  found
}

/// The coding agents installed on this machine, for the process provider to drive.
fn agent_commands() -> Vec<String> {
  const KNOWN: [&str; 14] = [
    "claude",
    "codex",
    "gemini",
    "opencode",
    "aider",
    "pi",
    "hermes",
    "droid",
    "amp",
    "grok",
    "cursor-agent",
    "copilot",
    "goose",
    "crush",
  ];
  let path = std::env::var_os("PATH").unwrap_or_default();
  let directories: Vec<_> = std::env::split_paths(&path).collect();
  KNOWN
    .iter()
    .filter(|command| {
      directories
        .iter()
        .any(|directory| directory.join(command).is_file())
    })
    .map(|command| (*command).to_string())
    .collect()
}

/// Endpoints a local model server usually listens on.
fn local_endpoints() -> Vec<String> {
  [
    "http://127.0.0.1:11434/v1",
    "http://127.0.0.1:4000/v1",
    "http://127.0.0.1:1234/v1",
    "http://127.0.0.1:8000/v1",
    "http://127.0.0.1:8080/v1",
  ]
  .iter()
  .map(|endpoint| (*endpoint).to_string())
  .collect()
}

pub async fn configure(config: &mut Config) -> Result<()> {
  let mut terminal = enter_terminal()?;
  let result = configure_inner(&mut terminal, config).await;
  leave_terminal(&mut terminal)?;

  match result? {
    Some((name, mut profile, model)) => {
      if !profile.models.contains(&model) {
        profile.models.push(model.clone());
        profile.models.sort();
      }
      profile.validate(&name)?;
      config.providers.insert(name.clone(), profile);
      config.provider = Some(name.clone());
      config.model = model.clone();
      config.save().await?;
      println!("configured {name} · {model}");
      Ok(())
    }
    None if config.validate().is_ok() => Ok(()),
    None => anyhow::bail!("setup cancelled before a provider was configured"),
  }
}

/// Offered once, after first-run setup, when a Synapse install is found. It is never turned on
/// without an answer: memory that reaches other tools is the user's call, not a default.
pub(crate) async fn offer_synapse(config: &mut Config) -> Result<bool> {
  let mut terminal = enter_terminal()?;
  let result = choose_synapse(&mut terminal);
  leave_terminal(&mut terminal)?;
  let accepted = result?;
  if accepted {
    config.synapse.enabled = true;
    config.memory.backend = ainz::MemoryBackend::Synapse;
    config.save().await?;
  }
  Ok(accepted)
}

fn choose_synapse(terminal: &mut Term) -> Result<bool> {
  let choices = [
    "Use Synapse for memory and guidance",
    "Keep memory local to this machine",
  ];
  let mut selected = 0;
  loop {
    terminal.draw(|frame| {
      let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(1),
      ])
      .areas(frame.area());
      render_header(frame, header, "Synapse found", ainz::synapse::SITE);
      let [list, detail] =
        Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
          .spacing(2)
          .areas(body);
      let mut state = ListState::default().with_selected(Some(selected));
      frame.render_stateful_widget(
        List::new(choices.map(ListItem::new))
          .block(
            Block::default()
              .title(" Memory ")
              .borders(Borders::ALL)
              .border_style(Style::default().fg(MUTED))
              .padding(ratatui::widgets::Padding::horizontal(1)),
          )
          .highlight_style(
            Style::default()
              .fg(Color::Black)
              .bg(ACCENT)
              .add_modifier(Modifier::BOLD),
          ),
        list,
        &mut state,
      );
      frame.render_widget(
        Paragraph::new(vec![
          Line::styled(
            ainz::synapse::SUMMARY,
            Style::default().fg(INK).add_modifier(Modifier::BOLD),
          ),
          Line::raw(""),
          Line::styled(
            "Ainz can keep what it works out in Synapse, so a later session here — or Claude \
             Code, or Codex — starts already knowing it. It also loads your SOUL.md guidance \
             and can put subagents on the Synapse mesh.",
            Style::default().fg(MUTED),
          ),
          Line::raw(""),
          Line::styled(
            "Either way this is a setting, not a commitment: /settings changes it whenever \
             you like, and Ainz runs the same without Synapse.",
            Style::default().fg(MUTED),
          ),
        ])
        .wrap(Wrap { trim: false })
        .block(
          Block::default()
            .title(" What this does ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MUTED))
            .padding(ratatui::widgets::Padding::new(2, 2, 1, 1)),
        ),
        detail,
      );
      render_footer(frame, footer, "↑↓ choose   enter confirm   esc keep local");
    })?;
    let key = match event::read()? {
      InputEvent::Mouse(mouse) => {
        match mouse.kind {
          MouseEventKind::ScrollUp => selected = selected.saturating_sub(1),
          MouseEventKind::ScrollDown => selected = (selected + 1).min(choices.len() - 1),
          MouseEventKind::Down(_) => {
            if let Some(index) = mouse.row.checked_sub(FIELDS_TOP).map(usize::from)
              && index < choices.len()
            {
              selected = index;
            }
          }
          _ => {}
        }
        continue;
      }
      InputEvent::Key(key) if key.kind != KeyEventKind::Release => key,
      _ => continue,
    };
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(choices.len() - 1),
      KeyCode::Enter => return Ok(selected == 0),
      KeyCode::Esc | KeyCode::Char('q') => return Ok(false),
      _ => {}
    }
  }
}

static PANIC_HOOK: Once = Once::new();
// what has to be undone on the way out, which differs when the app never left the main screen
static INLINE: AtomicBool = AtomicBool::new(false);

/// The prompt drawn at the bottom of the terminal's own scroll, leaving finished output in the
/// scrollback the terminal already keeps.
pub(super) fn enter_inline_terminal(rows: u16) -> Result<Term> {
  install_panic_hook();
  enable_raw_mode()?;
  INLINE.store(true, Ordering::Relaxed);
  let mut stdout = io::stdout();
  // no mouse capture here: the terminal's own scroll and selection are the point of inline
  if let Err(error) = execute!(stdout, EnableBracketedPaste) {
    restore_terminal();
    return Err(error.into());
  }
  push_keyboard_enhancement(&mut stdout);
  match Terminal::with_options(
    CrosstermBackend::new(stdout),
    TerminalOptions {
      viewport: Viewport::Inline(rows),
    },
  ) {
    Ok(terminal) => Ok(terminal),
    Err(error) => {
      restore_terminal();
      Err(error.into())
    }
  }
}

fn install_panic_hook() {
  // a panic must not leave the shell in raw mode on the alternate screen
  PANIC_HOOK.call_once(|| {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
      restore_terminal();
      previous(info);
    }));
  });
}

// ctrl+digit and ctrl+= only exist as distinct keys under the kitty keyboard protocol
fn push_keyboard_enhancement(stdout: &mut Stdout) {
  if matches!(
    crossterm::terminal::supports_keyboard_enhancement(),
    Ok(true)
  ) {
    drop(execute!(
      stdout,
      PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    ));
  }
}

pub(super) fn enter_terminal() -> Result<Term> {
  install_panic_hook();
  enable_raw_mode()?;
  INLINE.store(false, Ordering::Relaxed);
  let mut stdout = io::stdout();
  if let Err(error) = execute!(
    stdout,
    EnterAlternateScreen,
    EnableBracketedPaste,
    EnableMouseCapture
  ) {
    restore_terminal();
    return Err(error.into());
  }
  push_keyboard_enhancement(&mut stdout);
  match Terminal::new(CrosstermBackend::new(stdout)) {
    Ok(terminal) => Ok(terminal),
    Err(error) => {
      restore_terminal();
      Err(error.into())
    }
  }
}

pub(super) fn leave_terminal(terminal: &mut Term) -> Result<()> {
  restore_terminal();
  terminal.show_cursor()?;
  Ok(())
}

fn restore_terminal() {
  let mut stdout = io::stdout();
  drop(execute!(stdout, PopKeyboardEnhancementFlags));
  drop(execute!(stdout, DisableMouseCapture, DisableBracketedPaste));
  if !INLINE.load(Ordering::Relaxed) {
    drop(execute!(stdout, LeaveAlternateScreen));
  }
  drop(disable_raw_mode());
}

async fn configure_inner(
  terminal: &mut Term,
  config: &Config,
) -> Result<Option<(String, ProviderConfig, String)>> {
  let mut choices = vec![
    Choice::Preset(ProviderPreset::Ollama),
    Choice::Preset(ProviderPreset::LiteLlm),
    Choice::Preset(ProviderPreset::Codex),
    Choice::Preset(ProviderPreset::ClaudeCode),
  ];
  choices.extend(config.providers.keys().cloned().map(Choice::Existing));
  choices.extend([Choice::Http, Choice::Process]);
  let Some(choice) = select_provider(terminal, config, &choices)? else {
    return Ok(None);
  };

  let (name, profile, direct_model) = match choice {
    Choice::Preset(ProviderPreset::Ollama) => {
      let mut profile = preset_profile(ProviderPreset::Ollama);
      terminal.draw(|frame| render_loading(frame, "Finding local models…"))?;
      let provider = HttpProvider::new(
        profile
          .endpoint
          .clone()
          .context("HTTP provider requires an endpoint")?,
        String::new(),
        None,
      )?;
      if let Ok(models) = provider.models().await {
        profile.models = models;
      }
      ("ollama".into(), profile, None)
    }
    Choice::Preset(ProviderPreset::LiteLlm) => {
      let Some(endpoint) = choose_value(
        terminal,
        "LiteLLM proxy",
        "Where the proxy is listening",
        "Endpoint",
        local_endpoints(),
        "http://127.0.0.1:4000/v1",
      )?
      else {
        return Ok(None);
      };
      let Some(variable) = choose_value(
        terminal,
        "LiteLLM proxy",
        "The environment variable holding the key; the key itself is never stored",
        "API key environment variable",
        credential_variables(),
        "LITELLM_API_KEY",
      )?
      else {
        return Ok(None);
      };
      let mut profile = ProviderConfig::http(&endpoint, &variable);
      terminal.draw(|frame| render_loading(frame, "Asking the proxy which models it serves…"))?;
      let key = std::env::var(&variable).ok().filter(|key| !key.is_empty());
      if let Ok(provider) = HttpProvider::new(endpoint, String::new(), key)
        && let Ok(models) = provider.models().await
      {
        profile.models = models;
      }
      ("litellm".into(), profile, None)
    }
    Choice::Preset(ProviderPreset::Codex) => {
      let mut profile = preset_profile(ProviderPreset::Codex);
      profile.models = configured_model(".codex/config.toml", "model")
        .into_iter()
        .collect();
      ("codex".into(), profile, None)
    }
    Choice::Preset(ProviderPreset::ClaudeCode) => {
      let mut profile = preset_profile(ProviderPreset::ClaudeCode);
      profile.models = vec!["fable".into(), "opus".into(), "sonnet".into()];
      if let Some(model) = configured_model(".claude/settings.json", "model")
        && !profile.models.contains(&model)
      {
        profile.models.push(model);
      }
      ("claude".into(), profile, None)
    }
    Choice::Existing(name) => {
      let profile = config.providers[&name].clone();
      (name, profile, None)
    }
    Choice::Http => {
      let Some(endpoint) = choose_value(
        terminal,
        "Custom HTTP provider",
        "A chat-completions endpoint",
        "Endpoint",
        local_endpoints(),
        "http://127.0.0.1:11434/v1",
      )?
      else {
        return Ok(None);
      };
      let mut variables = credential_variables();
      variables.insert(0, "None".into());
      let Some(variable) = choose_value(
        terminal,
        "Custom HTTP provider",
        "The environment variable holding the key; the key itself is never stored",
        "API key environment variable",
        variables,
        "",
      )?
      else {
        return Ok(None);
      };
      let Some(values) = edit_fields(
        terminal,
        "Custom HTTP provider",
        vec![Field::new("Name", "http")],
      )?
      else {
        return Ok(None);
      };
      let profile = ProviderConfig::http(
        &endpoint,
        match variable.as_str() {
          "None" => "",
          variable => variable,
        },
      );
      (values[0].clone(), profile, None)
    }
    Choice::Process => {
      let Some(command) = choose_value(
        terminal,
        "Custom process provider",
        "The coding agents found on this machine",
        "Command",
        agent_commands(),
        "",
      )?
      else {
        return Ok(None);
      };
      let Some(values) = edit_fields(
        terminal,
        "Custom process provider",
        vec![
          Field::new("Name", "process"),
          Field::new("Command", &command),
          Field::new("Arguments", ""),
          Field::new("Model", ""),
          Field::new("Output: text, json, or stream?", "text"),
        ],
      )?
      else {
        return Ok(None);
      };
      let output = match values[4].trim().to_ascii_lowercase().as_str() {
        "json" | "j" | "y" | "yes" => ProcessOutput::JsonResult,
        "stream" | "s" | "stream-json" => ProcessOutput::StreamJson,
        _ => ProcessOutput::Text,
      };
      let profile = ProviderConfig::process(
        &values[1],
        values[2].split_whitespace().map(str::to_string).collect(),
        output,
      );
      (values[0].clone(), profile, Some(values[3].clone()))
    }
  };

  let model = match direct_model {
    Some(model) if !model.is_empty() => model,
    _ => match select_model(terminal, config, &name, &profile).await? {
      Some(model) => model,
      None => return Ok(None),
    },
  };
  Ok(Some((name, profile, model)))
}

fn select_provider(
  terminal: &mut Term,
  config: &Config,
  choices: &[Choice],
) -> Result<Option<Choice>> {
  let mut selected = 0;
  loop {
    terminal.draw(|frame| render_provider(frame, config, choices, selected))?;
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(choices.len() - 1),
      KeyCode::Enter => return Ok(Some(choices[selected].clone())),
      KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
      _ => {}
    }
  }
}

async fn select_model(
  terminal: &mut Term,
  config: &Config,
  name: &str,
  provider: &ProviderConfig,
) -> Result<Option<String>> {
  let mut known = provider.models.clone();
  // an endpoint knows what it serves, so ask it rather than making the name be remembered
  if known.is_empty() && provider.kind == ProviderKind::Http {
    terminal.draw(|frame| render_loading(frame, "Asking the endpoint which models it serves…"))?;
    if let Some(endpoint) = provider.endpoint.clone()
      && let Ok(discovered) = discover_models(config, provider, endpoint).await
    {
      known = discovered;
    }
  }
  if known.is_empty() {
    return Ok(
      edit_fields(terminal, "Choose a model", vec![Field::new("Model", "")])?
        .map(|values| values[0].clone()),
    );
  }
  let mut models = known;
  models.push("Enter another model…".into());
  let mut selected = if config.provider.as_deref() == Some(name) {
    models
      .iter()
      .position(|model| model == &config.model)
      .unwrap_or(0)
  } else {
    0
  };
  loop {
    terminal.draw(|frame| render_model(frame, name, &models, selected))?;
    let key = match event::read()? {
      InputEvent::Mouse(mouse) => {
        match mouse.kind {
          MouseEventKind::ScrollUp => selected = selected.saturating_sub(1),
          MouseEventKind::ScrollDown => selected = (selected + 1).min(models.len() - 1),
          MouseEventKind::Down(_) => {
            if let Some(index) = mouse.row.checked_sub(FIELDS_TOP).map(usize::from)
              && index < models.len()
            {
              selected = index;
            }
          }
          _ => {}
        }
        continue;
      }
      InputEvent::Key(key) if key.kind != KeyEventKind::Release => key,
      _ => continue,
    };
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(models.len() - 1),
      KeyCode::Enter if selected == models.len() - 1 => {
        return Ok(
          edit_fields(terminal, "Choose a model", vec![Field::new("Model", "")])?
            .map(|values| values[0].clone()),
        );
      }
      KeyCode::Enter => return Ok(Some(models[selected].clone())),
      KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
      _ => {}
    }
  }
}

/// Where the first row of a boxed list or field sits: the header, then the box's top border.
const FIELDS_TOP: u16 = 4;

/// A bracketed paste arrives whole, newlines and all, and every input here is one line.
pub(super) fn flatten_paste(text: &str) -> String {
  text.replace(['\r', '\n'], " ")
}

#[derive(Clone)]
struct Field {
  label: &'static str,
  value: String,
}

impl Field {
  fn new(label: &'static str, value: &str) -> Self {
    Self {
      label,
      value: value.into(),
    }
  }
}

fn edit_fields(
  terminal: &mut Term,
  title: &str,
  mut fields: Vec<Field>,
) -> Result<Option<Vec<String>>> {
  let mut selected = 0;
  loop {
    terminal.draw(|frame| render_fields(frame, title, &fields, selected))?;
    // a terminal with bracketed paste sends the whole clipboard as one event rather than as
    // keystrokes, so a field that only reads keys silently swallows every paste
    let key = match event::read()? {
      InputEvent::Paste(text) => {
        fields[selected].value.push_str(flatten_paste(&text).trim());
        continue;
      }
      // each field is three rows tall with a row of space under it, so a click divides by four
      InputEvent::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(_)) => {
        if let Some(index) = mouse
          .row
          .checked_sub(FIELDS_TOP)
          .map(|row| row / 4)
          .map(usize::from)
          && index < fields.len()
        {
          selected = index;
        }
        continue;
      }
      InputEvent::Key(key) if key.kind != KeyEventKind::Release => key,
      _ => continue,
    };
    match key.code {
      KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
      // a prefilled field has to be clearable without holding backspace down
      KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
        fields[selected].value.clear();
      }
      KeyCode::Char(ch) => fields[selected].value.push(ch),
      KeyCode::Backspace => {
        fields[selected].value.pop();
      }
      KeyCode::Tab | KeyCode::Down => selected = (selected + 1) % fields.len(),
      KeyCode::BackTab | KeyCode::Up => selected = (selected + fields.len() - 1) % fields.len(),
      KeyCode::Enter if selected + 1 < fields.len() => selected += 1,
      KeyCode::Enter
        if fields.iter().all(|field| {
          field.label.contains("API key") || field.label == "Arguments" || !field.value.is_empty()
        }) =>
      {
        return Ok(Some(fields.into_iter().map(|field| field.value).collect()));
      }
      KeyCode::Esc => return Ok(None),
      _ => {}
    }
  }
}

fn render_provider(frame: &mut Frame, config: &Config, choices: &[Choice], selected: usize) {
  let [header, body, footer] = Layout::vertical([
    Constraint::Length(3),
    Constraint::Min(10),
    Constraint::Length(1),
  ])
  .areas(frame.area());
  render_header(
    frame,
    header,
    "Choose a provider",
    "Providers can be changed later with /config",
  );
  let [list, detail] = Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
    .spacing(2)
    .areas(body);
  let items = choices.iter().map(|choice| {
    let active = config.provider.as_deref() == choice.key();
    let line = if active {
      Line::from(vec![
        Span::raw(choice.name()),
        Span::styled("  active", Style::default().fg(ACTIVE)),
      ])
    } else {
      Line::raw(choice.name())
    };
    ListItem::new(line)
  });
  let mut state = ListState::default().with_selected(Some(selected));
  frame.render_stateful_widget(
    List::new(items)
      .block(
        Block::default()
          .title(" Providers ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(MUTED)),
      )
      .highlight_style(
        Style::default()
          .fg(Color::Black)
          .bg(ACCENT)
          .add_modifier(Modifier::BOLD),
      )
      .highlight_symbol("  "),
    list,
    &mut state,
  );
  let (kind, description) = choices[selected].detail();
  frame.render_widget(
    Paragraph::new(vec![
      Line::styled(
        choices[selected].name(),
        Style::default().fg(INK).add_modifier(Modifier::BOLD),
      ),
      Line::raw(""),
      Line::styled(kind, Style::default().fg(ACCENT)),
      Line::raw(""),
      Line::styled(description, Style::default().fg(MUTED)),
    ])
    .wrap(Wrap { trim: false })
    .block(
      Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1)),
    ),
    detail,
  );
  render_footer(frame, footer, "↑↓ navigate   enter select   esc cancel");
}

fn render_model(frame: &mut Frame, provider: &str, models: &[String], selected: usize) {
  let [header, body, footer] = Layout::vertical([
    Constraint::Length(3),
    Constraint::Min(8),
    Constraint::Length(1),
  ])
  .areas(frame.area());
  render_header(frame, header, "Choose a model", provider);
  let width = body.width.min(72);
  let area = centered(body, width, body.height);
  let mut state = ListState::default().with_selected(Some(selected));
  frame.render_stateful_widget(
    List::new(models.iter().map(|model| ListItem::new(model.as_str())))
      .block(
        Block::default()
          .title(" Models ")
          .borders(Borders::ALL)
          .border_style(Style::default().fg(MUTED))
          .padding(ratatui::widgets::Padding::horizontal(1)),
      )
      .highlight_style(
        Style::default()
          .fg(Color::Black)
          .bg(ACCENT)
          .add_modifier(Modifier::BOLD),
      ),
    area,
    &mut state,
  );
  render_footer(frame, footer, "↑↓ navigate   enter select   esc back");
}

fn render_fields(frame: &mut Frame, title: &str, fields: &[Field], selected: usize) {
  let [header, body, footer] = Layout::vertical([
    Constraint::Length(3),
    Constraint::Min(8),
    Constraint::Length(1),
  ])
  .areas(frame.area());
  render_header(
    frame,
    header,
    title,
    "Values are saved to the Ainz config file",
  );
  let area = centered(body, body.width.min(82), body.height);
  let constraints: Vec<_> = fields
    .iter()
    .map(|_| Constraint::Length(3))
    .chain([Constraint::Min(0)])
    .collect();
  let rows = Layout::vertical(constraints).spacing(1).split(area);
  for (index, field) in fields.iter().enumerate() {
    let border = if index == selected { ACCENT } else { MUTED };
    // a value that outgrows its box shows its end while it is being edited; one that fits
    // stays put, first character and all
    let scroll = if index == selected {
      let inner = rows[index].width.saturating_sub(4) as usize;
      u16::try_from(field.value.chars().count().saturating_sub(inner)).unwrap_or(u16::MAX)
    } else {
      0
    };
    frame.render_widget(
      Paragraph::new(field.value.as_str())
        .scroll((0, scroll))
        .style(Style::default().fg(INK))
        .block(
          Block::default()
            .title(format!(" {} ", field.label))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
      rows[index],
    );
  }
  render_footer(frame, footer, "tab move   enter next/save   esc cancel");
}

fn render_loading(frame: &mut Frame, message: &str) {
  let area = centered(frame.area(), frame.area().width.min(60), 5);
  frame.render_widget(
    Paragraph::new(message)
      .alignment(Alignment::Center)
      .style(Style::default().fg(ACCENT))
      .block(
        Block::default()
          .borders(Borders::ALL)
          .border_style(Style::default().fg(MUTED))
          .padding(ratatui::widgets::Padding::vertical(1)),
      ),
    area,
  );
}

fn render_header(frame: &mut Frame, area: Rect, title: &str, subtitle: &str) {
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(
        "Ainz  ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
      ),
      Span::styled(title, Style::default().fg(INK).add_modifier(Modifier::BOLD)),
      Span::styled(format!("  {subtitle}"), Style::default().fg(MUTED)),
    ])),
    area,
  );
}

fn render_footer(frame: &mut Frame, area: Rect, text: &str) {
  frame.render_widget(
    Paragraph::new(text)
      .alignment(Alignment::Center)
      .style(Style::default().fg(MUTED)),
    area,
  );
}

pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
  let [area] = Layout::horizontal([Constraint::Length(width)])
    .flex(ratatui::layout::Flex::Center)
    .areas(area);
  let [area] = Layout::vertical([Constraint::Length(height)])
    .flex(ratatui::layout::Flex::Center)
    .areas(area);
  area
}

#[cfg(test)]
mod tests {
  use ratatui::{Terminal, backend::TestBackend};

  use super::{Field, render_fields};

  fn rendered(fields: &[Field], selected: usize, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 14)).unwrap();
    terminal
      .draw(|frame| render_fields(frame, "Setup", fields, selected))
      .unwrap();
    terminal
      .backend()
      .buffer()
      .content()
      .iter()
      .map(ratatui::buffer::Cell::symbol)
      .collect()
  }

  #[test]
  fn a_focused_field_that_fits_shows_its_first_character() {
    let fields = vec![Field::new("Endpoint", "http://127.0.0.1:4000/v1")];

    assert!(rendered(&fields, 0, 70).contains("http://127.0.0.1:4000/v1"));
  }

  #[test]
  fn a_focused_field_longer_than_its_box_shows_its_end() {
    let key = format!("sk-{}-tail", "x".repeat(90));
    let fields = vec![Field::new("API key environment variable", &key)];

    let text = rendered(&fields, 0, 60);

    assert!(text.contains("-tail"));
    assert!(!text.contains("sk-x"));
  }
}
