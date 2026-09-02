use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ratatui::{
  style::{Color, Modifier, Style},
  text::{Line, Span},
};
use tokio::{fs, io::AsyncReadExt};

const MAX_BYTES: u64 = 128 * 1024;
const MAX_LINES: usize = 80;
const MAX_WIDTH: usize = 240;

#[derive(Clone, Debug)]
pub struct HeaderArt {
  pub name: String,
  pub path: PathBuf,
  pub lines: Vec<Line<'static>>,
  pub width: usize,
}

#[derive(Clone, Debug, Default)]
pub struct HeaderCatalog {
  pub headers: Vec<HeaderArt>,
  pub issues: Vec<String>,
}

impl HeaderCatalog {
  pub async fn discover(workspace: &Path) -> Result<Self> {
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
      roots.push(config.join("agentx/headers"));
      roots.push(config.join("ainz/headers"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    for path in ancestors {
      roots.push(path.join(".agentx/headers"));
      roots.push(path.join(".ainz/headers"));
    }

    let mut headers = BTreeMap::new();
    let mut issues = Vec::new();
    for root in roots {
      discover_root(&root, &mut headers, &mut issues).await?;
    }
    Ok(Self {
      headers: headers.into_values().collect(),
      issues,
    })
  }

  pub fn get(&self, name: &str) -> Option<&HeaderArt> {
    self.headers.iter().find(|header| header.name == name)
  }
}

async fn discover_root(
  root: &Path,
  headers: &mut BTreeMap<String, HeaderArt>,
  issues: &mut Vec<String>,
) -> Result<()> {
  // a bad headers directory is reported next to bad art rather than blocking the chat
  let mut entries = match fs::read_dir(root).await {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
    Err(error) => {
      issues.push(format!("{}: {error}", root.display()));
      return Ok(());
    }
  };
  while let Some(entry) = entries.next_entry().await? {
    let path = entry.path();
    if !entry.file_type().await?.is_file() || !supported(&path) {
      continue;
    }
    let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
      continue;
    };
    if !valid_name(name) {
      issues.push(format!(
        "{}: filename must use letters, numbers, - or _",
        path.display()
      ));
      continue;
    }
    match load(&path, name).await {
      Ok(header) => {
        headers.insert(name.into(), header);
      }
      Err(error) => issues.push(format!("{}: {error:#}", path.display())),
    }
  }
  Ok(())
}

fn supported(path: &Path) -> bool {
  path
    .extension()
    .and_then(|extension| extension.to_str())
    .is_some_and(|extension| {
      matches!(
        extension.to_ascii_lowercase().as_str(),
        "ans" | "ansi" | "txt"
      )
    })
}

fn valid_name(name: &str) -> bool {
  !name.is_empty()
    && name
      .chars()
      .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

async fn load(path: &Path, name: &str) -> Result<HeaderArt> {
  let mut bytes = Vec::new();
  fs::File::open(path)
    .await?
    .take(MAX_BYTES + 1)
    .read_to_end(&mut bytes)
    .await?;
  if bytes.len() as u64 > MAX_BYTES {
    bail!("header exceeds the {MAX_BYTES} byte limit");
  }
  let text = std::str::from_utf8(&bytes).context("header must be UTF-8")?;
  let lines = parse_ansi(text)?;
  if lines.is_empty() {
    bail!("header is empty");
  }
  if lines.len() > MAX_LINES {
    bail!("header exceeds the {MAX_LINES} line limit");
  }
  let width = lines.iter().map(Line::width).max().unwrap_or_default();
  if width > MAX_WIDTH {
    bail!("header is {width} columns wide; maximum is {MAX_WIDTH}");
  }
  Ok(HeaderArt {
    name: name.into(),
    path: path.to_path_buf(),
    lines,
    width,
  })
}

fn parse_ansi(input: &str) -> Result<Vec<Line<'static>>> {
  let mut lines = Vec::new();
  let mut spans = Vec::new();
  let mut text = String::new();
  let mut style = Style::default();
  let mut chars = input.chars().peekable();
  while let Some(character) = chars.next() {
    match character {
      '\n' => {
        flush(&mut spans, &mut text, style);
        lines.push(Line::from(std::mem::take(&mut spans)));
      }
      '\r' if chars.peek() == Some(&'\n') => {}
      '\t' => text.push_str("  "),
      '\u{1b}' => {
        flush(&mut spans, &mut text, style);
        if chars.next() != Some('[') {
          bail!("only ANSI SGR color sequences are allowed");
        }
        let mut parameters = String::new();
        loop {
          match chars.next() {
            Some('m') => break,
            Some(value) if value.is_ascii_digit() || value == ';' => parameters.push(value),
            _ => bail!("only ANSI SGR color sequences are allowed"),
          }
          if parameters.len() > 64 {
            bail!("ANSI sequence is too long");
          }
        }
        style = apply_sgr(style, &parameters)?;
      }
      value if value.is_control() => {
        bail!("unsupported control character U+{:04X}", value as u32)
      }
      value => text.push(value),
    }
  }
  flush(&mut spans, &mut text, style);
  if !spans.is_empty() {
    lines.push(Line::from(spans));
  }
  while lines.last().is_some_and(|line| line.width() == 0) {
    lines.pop();
  }
  Ok(lines)
}

fn flush(spans: &mut Vec<Span<'static>>, text: &mut String, style: Style) {
  if !text.is_empty() {
    spans.push(Span::styled(std::mem::take(text), style));
  }
}

fn apply_sgr(mut style: Style, parameters: &str) -> Result<Style> {
  let values = if parameters.is_empty() {
    vec![0]
  } else {
    parameters
      .split(';')
      .map(|value| value.parse::<u16>().context("invalid ANSI parameter"))
      .collect::<Result<Vec<_>>>()?
  };
  let mut index = 0;
  while index < values.len() {
    let value = values[index];
    match value {
      0 => style = Style::default(),
      1 => style = style.add_modifier(Modifier::BOLD),
      2 => style = style.add_modifier(Modifier::DIM),
      3 => style = style.add_modifier(Modifier::ITALIC),
      4 => style = style.add_modifier(Modifier::UNDERLINED),
      7 => style = style.add_modifier(Modifier::REVERSED),
      9 => style = style.add_modifier(Modifier::CROSSED_OUT),
      22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
      23 => style = style.remove_modifier(Modifier::ITALIC),
      24 => style = style.remove_modifier(Modifier::UNDERLINED),
      27 => style = style.remove_modifier(Modifier::REVERSED),
      29 => style = style.remove_modifier(Modifier::CROSSED_OUT),
      30..=37 | 90..=97 => style = style.fg(ansi_color(value)),
      39 => style = style.fg(Color::Reset),
      40..=47 | 100..=107 => style = style.bg(ansi_color(value - 10)),
      49 => style = style.bg(Color::Reset),
      38 | 48 => {
        let foreground = value == 38;
        let color = extended_color(&values, &mut index)?;
        style = if foreground {
          style.fg(color)
        } else {
          style.bg(color)
        };
      }
      _ => bail!("unsupported ANSI SGR code {value}"),
    }
    index += 1;
  }
  Ok(style)
}

fn extended_color(values: &[u16], index: &mut usize) -> Result<Color> {
  match values.get(*index + 1) {
    Some(5) => {
      let color = *values
        .get(*index + 2)
        .context("incomplete 256-color sequence")?;
      if color > 255 {
        bail!("256-color index must be between 0 and 255");
      }
      *index += 2;
      Ok(Color::Indexed(color as u8))
    }
    Some(2) => {
      let channels = values
        .get(*index + 2..=*index + 4)
        .context("incomplete truecolor sequence")?;
      if channels.iter().any(|channel| *channel > 255) {
        bail!("truecolor channels must be between 0 and 255");
      }
      *index += 4;
      Ok(Color::Rgb(
        channels[0] as u8,
        channels[1] as u8,
        channels[2] as u8,
      ))
    }
    _ => bail!("extended color must use 5;n or 2;r;g;b"),
  }
}

fn ansi_color(value: u16) -> Color {
  match value {
    30 => Color::Black,
    31 => Color::Red,
    32 => Color::Green,
    33 => Color::Yellow,
    34 => Color::Blue,
    35 => Color::Magenta,
    36 => Color::Cyan,
    37 => Color::Gray,
    90 => Color::DarkGray,
    91 => Color::LightRed,
    92 => Color::LightGreen,
    93 => Color::LightYellow,
    94 => Color::LightBlue,
    95 => Color::LightMagenta,
    96 => Color::LightCyan,
    97 => Color::White,
    _ => Color::Reset,
  }
}
