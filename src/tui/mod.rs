use std::{
  io::{self, Stdout},
  sync::Once,
};

use ainz::{Config, HttpProvider, ProcessOutput, ProviderConfig};
use anyhow::{Context, Result};
use crossterm::{
  event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as InputEvent, KeyCode, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
  },
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
  Frame, Terminal,
  backend::CrosstermBackend,
  layout::{Alignment, Constraint, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::command::{ProviderPreset, preset_profile};

mod chat;
mod import;
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
        "Uses print mode with structured output. The CLI keeps authority over its own tools.",
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
        "Send the transcript over stdin and use stdout, or a JSON result field, as the response.",
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
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
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

pub(super) fn enter_terminal() -> Result<Term> {
  // a panic must not leave the shell in raw mode on the alternate screen
  PANIC_HOOK.call_once(|| {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
      restore_terminal();
      previous(info);
    }));
  });
  enable_raw_mode()?;
  let mut stdout = io::stdout();
  if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
    restore_terminal();
    return Err(error.into());
  }
  // ctrl+digit and ctrl+= only exist as distinct keys under the kitty keyboard protocol
  if matches!(
    crossterm::terminal::supports_keyboard_enhancement(),
    Ok(true)
  ) {
    drop(execute!(
      stdout,
      PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    ));
  }
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
  drop(execute!(
    stdout,
    DisableBracketedPaste,
    LeaveAlternateScreen
  ));
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
      let Some(values) = edit_fields(
        terminal,
        "LiteLLM proxy",
        vec![
          Field::new("Name", "litellm"),
          Field::new("Endpoint", "http://127.0.0.1:4000/v1"),
          Field::new("API key environment variable", "LITELLM_API_KEY"),
        ],
      )?
      else {
        return Ok(None);
      };
      let mut profile = ProviderConfig::http(&values[1], &values[2]);
      terminal.draw(|frame| render_loading(frame, "Asking the proxy which models it serves…"))?;
      let key = std::env::var(&values[2]).ok().filter(|key| !key.is_empty());
      if let Ok(provider) = HttpProvider::new(values[1].clone(), String::new(), key)
        && let Ok(models) = provider.models().await
      {
        profile.models = models;
      }
      (values[0].clone(), profile, None)
    }
    Choice::Preset(ProviderPreset::Codex) => {
      ("codex".into(), preset_profile(ProviderPreset::Codex), None)
    }
    Choice::Preset(ProviderPreset::ClaudeCode) => {
      let mut profile = preset_profile(ProviderPreset::ClaudeCode);
      profile.models = vec!["sonnet".into(), "opus".into()];
      ("claude".into(), profile, None)
    }
    Choice::Existing(name) => {
      let profile = config.providers[&name].clone();
      (name, profile, None)
    }
    Choice::Http => {
      let Some(values) = edit_fields(
        terminal,
        "Custom HTTP provider",
        vec![
          Field::new("Name", "http"),
          Field::new("Endpoint", "http://127.0.0.1:11434/v1"),
          Field::new("API key environment variable", ""),
          Field::new("Model", ""),
        ],
      )?
      else {
        return Ok(None);
      };
      let profile = ProviderConfig::http(&values[1], &values[2]);
      (values[0].clone(), profile, Some(values[3].clone()))
    }
    Choice::Process => {
      let Some(values) = edit_fields(
        terminal,
        "Custom process provider",
        vec![
          Field::new("Name", "process"),
          Field::new("Command", ""),
          Field::new("Arguments", ""),
          Field::new("Model", ""),
          Field::new("JSON result field?", "no"),
        ],
      )?
      else {
        return Ok(None);
      };
      let output = if matches!(values[4].as_str(), "y" | "yes") {
        ProcessOutput::JsonResult
      } else {
        ProcessOutput::Text
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
    _ => match select_model(terminal, config, &name, &profile)? {
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

fn select_model(
  terminal: &mut Term,
  config: &Config,
  name: &str,
  provider: &ProviderConfig,
) -> Result<Option<String>> {
  if provider.models.is_empty() {
    return Ok(
      edit_fields(terminal, "Choose a model", vec![Field::new("Model", "")])?
        .map(|values| values[0].clone()),
    );
  }
  let mut models = provider.models.clone();
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
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
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
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
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
    frame.render_widget(
      Paragraph::new(field.value.as_str())
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
