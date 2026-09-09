use std::collections::BTreeMap;

use ainz::mcp::valid_name;
use ainz::{McpProfile, McpServerConfig, McpTransport};
use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event as InputEvent, KeyCode, KeyEventKind};
use ratatui::{
  Frame,
  layout::{Alignment, Constraint, Layout, Rect},
  style::Modifier,
  text::{Line, Span},
  widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use super::{CommandData, Entry, EntryKind};

pub(super) async fn command(
  terminal: &mut super::super::Term,
  input: &str,
  state: &mut super::ChatState,
  command_data: &mut CommandData,
) -> Result<bool> {
  if !input.starts_with("/mcp ")
    && input != "/mcp"
    && input != "/mcp list"
    && input != "/mcp import"
  {
    return Ok(false);
  }
  if input == "/mcp import" {
    return Ok(false);
  }
  if input == "/mcp" || input == "/mcp list" {
    manage(terminal, state, command_data).await?;
    return Ok(true);
  }
  let rest = input.get(5..).unwrap_or_default();
  let mut parts = rest.split_whitespace();
  let action = parts
    .next()
    .context("usage: /mcp [add NAME --required -- COMMAND | remove NAME]")?;
  match action {
    "add" => {
      let remainder = parts.collect::<Vec<_>>();
      if remainder.is_empty() {
        return add_interactive(terminal, state, command_data).await;
      }
      let (name, required, command) = parse_add_args(&remainder)?;
      add_named(state, command_data, &name, required, command).await
    }
    "remove" => {
      let name = parts.next().context("usage: /mcp remove NAME")?;
      if !parts.next().is_none() {
        bail!("usage: /mcp remove NAME");
      }
      remove_named(state, command_data, name).await
    }
    _ => bail!("unknown MCP command: use /mcp add and /mcp remove"),
  }
}

async fn manage(
  terminal: &mut super::super::Term,
  state: &mut super::ChatState,
  command_data: &mut CommandData,
) -> Result<()> {
  let profile = McpProfile::load().await?;
  if profile.servers.is_empty() {
    state.primary.entries.push(Entry::new(
      EntryKind::System,
      format!(
        "Manage MCP servers\n0 servers\n\nNo servers configured. Use /mcp add to connect one.\n\n{}",
        profile_path()
      ),
    ));
    return Ok(());
  }
  let names: Vec<String> = profile.servers.keys().cloned().collect();
  let mut selected = 0usize;
  loop {
    terminal.draw(|frame| render_list(frame, &profile, &names, selected))?;
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(names.len() - 1),
      KeyCode::Enter => {
        let name = &names[selected];
        detail(terminal, state, command_data, name).await?;
        return Ok(());
      }
      KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
      _ => {}
    }
  }
}

async fn detail(
  terminal: &mut super::super::Term,
  state: &mut super::ChatState,
  command_data: &mut CommandData,
  name: &str,
) -> Result<()> {
  let mut profile = McpProfile::load().await?;
  let mut action = 0usize;
  loop {
    let Some(config) = profile.servers.get(name) else {
      return Ok(());
    };
    terminal.draw(|frame| render_detail(frame, name, config, action))?;
    let InputEvent::Key(key) = event::read()? else {
      continue;
    };
    if key.kind == KeyEventKind::Release {
      continue;
    }
    match key.code {
      KeyCode::Up | KeyCode::Char('k') => action = action.saturating_sub(1),
      KeyCode::Down | KeyCode::Char('j') => action = (action + 1).min(2),
      KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
      KeyCode::Enter => match action {
        0 => {
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            format!("MCP server {name}: tool discovery is available when the server is called."),
          ));
          return Ok(());
        }
        1 => {
          if let Some(config) = profile.servers.get_mut(name) {
            config.enabled = !config.enabled;
          }
          profile.save().await?;
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            format!(
              "MCP server {name} {}",
              if profile.servers[name].enabled {
                "enabled"
              } else {
                "disabled"
              }
            ),
          ));
          if !profile.servers[name].enabled {
            command_data.mcp.retain(|entry| entry != name);
          } else if !command_data.mcp.iter().any(|entry| entry == name) {
            command_data.mcp.push(name.to_string());
            command_data.mcp.sort();
          }
          return Ok(());
        }
        _ => {
          profile.servers.remove(name);
          profile.save().await?;
          command_data.mcp.retain(|entry| entry != name);
          state.primary.entries.push(Entry::new(
            EntryKind::System,
            format!("removed mcp server {name}"),
          ));
          return Ok(());
        }
      },
      _ => {}
    }
  }
}

fn render_list(frame: &mut Frame, profile: &McpProfile, names: &[String], selected: usize) {
  let area = frame.area();
  frame.render_widget(Clear, area);
  let block = Block::default()
    .title(Span::styled(
      " Manage MCP servers ",
      super::super::Style::default()
        .fg(super::super::ACTIVE)
        .add_modifier(Modifier::BOLD),
    ))
    .borders(Borders::ALL)
    .border_style(super::super::Style::default().fg(super::super::ACTIVE));
  let inner = block.inner(area);
  frame.render_widget(block, area);
  let chunks = Layout::vertical([
    Constraint::Length(2),
    Constraint::Min(3),
    Constraint::Length(2),
  ])
  .split(inner);
  frame.render_widget(
    Paragraph::new(format!("{} servers  ·  {}", names.len(), profile_path()))
      .style(super::super::Style::default().fg(super::super::MUTED)),
    chunks[0],
  );
  let items = names.iter().enumerate().map(|(index, name)| {
    let config = &profile.servers[name];
    let marker = if index == selected { "›" } else { " " };
    let state = if config.enabled {
      "✓ enabled"
    } else {
      "× disabled"
    };
    let transport = match config.transport {
      McpTransport::Stdio => "stdio",
      McpTransport::StreamableHttp => "http",
    };
    ListItem::new(Line::from(vec![
      Span::styled(
        format!("{marker} "),
        super::super::Style::default().fg(super::super::ACTIVE),
      ),
      Span::styled(
        format!("{name:<24}"),
        super::super::Style::default().fg(super::super::INK),
      ),
      Span::styled(
        format!(" · {state} · {transport}"),
        super::super::Style::default().fg(super::super::MUTED),
      ),
    ]))
  });
  frame.render_widget(List::new(items), chunks[1]);
  frame.render_widget(
    Paragraph::new("↑/↓ navigate · Enter inspect · Esc close")
      .style(super::super::Style::default().fg(super::super::MUTED)),
    chunks[2],
  );
}

fn render_detail(frame: &mut Frame, name: &str, config: &McpServerConfig, selected: usize) {
  let area = frame.area();
  frame.render_widget(Clear, area);
  let block = Block::default()
    .borders(Borders::ALL)
    .border_style(super::super::Style::default().fg(super::super::ACTIVE));
  let inner = block.inner(area);
  frame.render_widget(block, area);
  let chunks = Layout::vertical([
    Constraint::Length(1),
    Constraint::Min(8),
    Constraint::Length(6),
  ])
  .split(inner);
  frame.render_widget(
    Paragraph::new(format!("{name} MCP server")).style(
      super::super::Style::default()
        .fg(super::super::ACTIVE)
        .add_modifier(Modifier::BOLD),
    ),
    chunks[0],
  );
  let protocol = match config.transport {
    McpTransport::Stdio => "stdio",
    McpTransport::StreamableHttp => "streamable HTTP",
  };
  let endpoint = config.url.as_deref().unwrap_or_else(|| {
    config
      .command
      .first()
      .map(String::as_str)
      .unwrap_or("(no command)")
  });
  let info = vec![
    Line::from(vec![
      Span::styled(
        "Status:       ",
        super::super::Style::default().add_modifier(Modifier::BOLD),
      ),
      Span::raw(if config.enabled {
        "✓ enabled"
      } else {
        "× disabled"
      }),
    ]),
    Line::from(vec![
      Span::styled(
        "Protocol:     ",
        super::super::Style::default().add_modifier(Modifier::BOLD),
      ),
      Span::raw(protocol),
    ]),
    Line::from(vec![
      Span::styled(
        "Endpoint:     ",
        super::super::Style::default().add_modifier(Modifier::BOLD),
      ),
      Span::raw(endpoint),
    ]),
    Line::from(vec![
      Span::styled(
        "Required:     ",
        super::super::Style::default().add_modifier(Modifier::BOLD),
      ),
      Span::raw(if config.required { "yes" } else { "no" }),
    ]),
    Line::from(vec![
      Span::styled(
        "Timeout:      ",
        super::super::Style::default().add_modifier(Modifier::BOLD),
      ),
      Span::raw(format!("{} ms", config.timeout_ms)),
    ]),
  ];
  frame.render_widget(Paragraph::new(info).wrap(Wrap { trim: false }), chunks[1]);
  let actions = [
    "View tools",
    if config.enabled { "Disable" } else { "Enable" },
    "Remove",
  ];
  let items = actions.iter().enumerate().map(|(index, label)| {
    ListItem::new(Line::from(vec![
      Span::styled(
        if index == selected { "› " } else { "  " },
        super::super::Style::default().fg(super::super::ACTIVE),
      ),
      Span::styled(
        *label,
        if index == selected {
          super::super::Style::default().fg(super::super::ACTIVE)
        } else {
          super::super::Style::default().fg(super::super::INK)
        },
      ),
    ]))
  });
  frame.render_widget(List::new(items), chunks[2]);
  frame.render_widget(
    Paragraph::new("↑/↓ navigate · Enter select · Esc back")
      .alignment(Alignment::Right)
      .style(super::super::Style::default().fg(super::super::MUTED)),
    Rect {
      x: inner.x,
      y: inner.bottom().saturating_sub(1),
      width: inner.width,
      height: 1,
    },
  );
}

fn profile_path() -> String {
  McpProfile::path()
    .map(|path| path.display().to_string())
    .unwrap_or_else(|_| "profile unavailable".into())
}

async fn add_named(
  state: &mut super::ChatState,
  command_data: &mut CommandData,
  name: &str,
  required: bool,
  command: Vec<String>,
) -> Result<bool> {
  add_profile(name, required, command).await?;
  if !command_data.mcp.iter().any(|entry| entry == name) {
    command_data.mcp.push(name.to_string());
    command_data.mcp.sort();
  }
  state.primary.entries.push(Entry::new(
    EntryKind::System,
    format!("added mcp server {name}"),
  ));
  Ok(true)
}

async fn add_interactive(
  terminal: &mut super::super::Term,
  state: &mut super::ChatState,
  command_data: &mut CommandData,
) -> Result<bool> {
  let values = super::super::edit_fields(
    terminal,
    "Add MCP server",
    vec![
      super::super::Field::new("Name", ""),
      super::super::Field::new("Command", ""),
      super::super::Field::new("Required", "n"),
    ],
  )?
  .context("MCP add cancelled")?;
  let name = values[0].trim().to_string();
  let command = values[1].split_whitespace().map(str::to_string).collect();
  let required = matches!(
    values[2].trim().to_ascii_lowercase().as_str(),
    "y" | "yes" | "true" | "1" | "on"
  );
  add_named(state, command_data, &name, required, command).await
}

async fn remove_named(
  state: &mut super::ChatState,
  command_data: &mut CommandData,
  name: &str,
) -> Result<bool> {
  let mut profile = McpProfile::load().await?;
  if profile.servers.remove(name).is_none() {
    bail!("MCP server {name} is not configured");
  }
  profile.save().await?;
  command_data.mcp.retain(|entry| entry != name);
  state.primary.entries.push(Entry::new(
    EntryKind::System,
    format!("removed mcp server {name}"),
  ));
  Ok(true)
}

async fn add_profile(name: &str, required: bool, command: Vec<String>) -> Result<()> {
  if name.trim().is_empty() {
    bail!("usage: /mcp add NAME --required -- COMMAND");
  }
  if !valid_name(name) {
    bail!("MCP server name {name:?} may only use letters, digits, '.', '_' and '-'");
  }
  if command.is_empty() {
    bail!("usage: /mcp add NAME --required -- COMMAND");
  }
  let mut profile = McpProfile::load().await?;
  if profile.servers.contains_key(name) {
    bail!("MCP server {name} is already configured");
  }
  profile.servers.insert(
    name.to_string(),
    McpServerConfig {
      transport: McpTransport::Stdio,
      command,
      url: None,
      header_env: BTreeMap::new(),
      headers: BTreeMap::new(),
      env: BTreeMap::new(),
      cwd: None,
      enabled: true,
      required,
      timeout_ms: 30_000,
    },
  );
  profile.save().await
}

fn parse_add_args(parts: &[&str]) -> Result<(String, bool, Vec<String>)> {
  let Some(name) = parts.first() else {
    bail!("usage: /mcp add NAME --required -- COMMAND");
  };
  if name.trim().is_empty() {
    bail!("usage: /mcp add NAME --required -- COMMAND");
  }
  let mut required = false;
  let mut command = Vec::new();
  let mut separator = false;
  for arg in parts.iter().skip(1) {
    if !separator {
      match *arg {
        "--required" => required = true,
        "--" => separator = true,
        _ => bail!("use '--required' and '--' before the command"),
      }
    } else {
      command.push((*arg).to_string());
    }
  }
  if !separator {
    bail!("usage: /mcp add NAME --required -- COMMAND");
  }
  if command.is_empty() {
    bail!("usage: /mcp add NAME --required -- COMMAND");
  }
  Ok((name.to_string(), required, command))
}

#[cfg(test)]
mod tests {
  use super::parse_add_args;

  #[test]
  fn parses_required_stdio_server() {
    let parsed = parse_add_args(&["files", "--required", "--", "npx", "-y", "server"])
      .expect("valid MCP add arguments");
    assert_eq!(parsed.0, "files");
    assert!(parsed.1);
    assert_eq!(parsed.2, ["npx", "-y", "server"]);
  }

  #[test]
  fn rejects_missing_command_separator() {
    assert!(parse_add_args(&["files", "npx", "server"]).is_err());
  }
}
