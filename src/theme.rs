use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use serde::Deserialize;
use tokio::{fs, io::AsyncReadExt};

pub const COLORS: [(&str, Color); 13] = [
  ("background", Color::Reset),
  ("text", Color::Rgb(218, 222, 226)),
  ("muted", Color::Rgb(128, 138, 148)),
  ("accent", Color::Rgb(83, 196, 190)),
  ("success", Color::Rgb(145, 210, 138)),
  ("bar", Color::Rgb(24, 66, 128)),
  ("info", Color::Rgb(72, 205, 214)),
  ("warning", Color::Rgb(230, 199, 92)),
  ("error", Color::Rgb(224, 103, 103)),
  ("special", Color::Rgb(198, 118, 205)),
  ("bright", Color::White),
  ("bar_text", Color::White),
  ("border", Color::Rgb(24, 66, 128)),
];

#[derive(Clone, Debug)]
pub struct Theme {
  pub name: String,
  pub path: PathBuf,
  colors: [Color; 13],
}

impl Default for Theme {
  fn default() -> Self {
    Self {
      name: "default".into(),
      path: PathBuf::new(),
      colors: COLORS.map(|(_, color)| color),
    }
  }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
  colors: BTreeMap<String, String>,
}

impl Theme {
  pub fn parse(name: &str, text: &str) -> Result<Self> {
    let file: File = toml::from_str(text).context("read theme TOML")?;
    let mut theme = Self {
      name: name.into(),
      ..Self::default()
    };
    for (role, value) in file.colors {
      let index = COLORS
        .iter()
        .position(|(name, _)| *name == role)
        .with_context(|| format!("unknown theme color {role}"))?;
      theme.colors[index] = if value == "default" && role == "background" {
        Color::Reset
      } else if value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
      {
        let rgb = u32::from_str_radix(&value[1..], 16)?;
        Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
      } else {
        bail!("{role} must be #RRGGBB; background also accepts default");
      };
    }
    Ok(theme)
  }

  pub fn color(&self, role: &str) -> Option<Color> {
    COLORS
      .iter()
      .position(|(name, _)| *name == role)
      .map(|index| self.colors[index])
  }

  pub fn apply(&self, buffer: &mut Buffer, artwork: Option<Rect>) {
    if self.name == "default" {
      return;
    }
    let area = buffer.area;
    for (index, cell) in buffer.content.iter_mut().enumerate() {
      let x = area.x + (index % usize::from(area.width.max(1))) as u16;
      let y = area.y + (index / usize::from(area.width.max(1))) as u16;
      if artwork.is_some_and(|art| art.contains((x, y).into())) {
        if cell.fg == Color::Reset {
          cell.fg = self.colors[1];
        }
        if cell.bg == Color::Reset {
          cell.bg = self.colors[0];
        }
        continue;
      }
      let bar = cell.bg == COLORS[5].1;
      cell.fg = if bar {
        self.colors[11]
      } else if cell.fg == Color::Reset {
        self.colors[1]
      } else if cell.fg == COLORS[5].1 {
        self.colors[12]
      } else {
        COLORS[1..=10]
          .iter()
          .position(|(_, color)| *color == cell.fg)
          .map(|index| self.colors[index + 1])
          .unwrap_or(cell.fg)
      };
      cell.bg = if bar {
        self.colors[5]
      } else if matches!(cell.bg, Color::Reset | Color::Black) {
        self.colors[0]
      } else {
        cell.bg
      };
    }
  }
}

#[derive(Default)]
pub struct ThemeCatalog {
  pub themes: Vec<Theme>,
  pub issues: Vec<String>,
}

impl ThemeCatalog {
  pub fn get(&self, name: &str) -> Option<&Theme> {
    self.themes.iter().find(|theme| theme.name == name)
  }

  pub async fn discover(workspace: &Path) -> Result<Self> {
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
      roots.push(config.join("ainz/themes"));
    }
    let mut ancestors: Vec<_> = workspace.ancestors().collect();
    ancestors.reverse();
    roots.extend(ancestors.into_iter().map(|path| path.join(".ainz/themes")));
    let mut themes = BTreeMap::new();
    let mut issues = Vec::new();
    for root in roots {
      let mut entries = match fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
        Err(error) => {
          issues.push(format!("{}: {error}", root.display()));
          continue;
        }
      };
      while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await?.is_file() || path.extension().is_none_or(|ext| ext != "toml") {
          continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
          continue;
        };
        if name == "default"
          || name.is_empty()
          || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
          issues.push(format!(
            "{}: invalid or reserved theme name",
            path.display()
          ));
          continue;
        }
        let result = async {
          let mut bytes = Vec::new();
          fs::File::open(&path)
            .await?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
          if bytes.len() > 16 * 1024 {
            bail!("theme exceeds 16 KiB");
          }
          Theme::parse(name, std::str::from_utf8(&bytes)?)
        }
        .await;
        match result {
          Ok(mut theme) => {
            theme.path = path.clone();
            themes.insert(name.to_string(), theme);
          }
          Err(error) => issues.push(format!("{}: {error:#}", path.display())),
        }
      }
    }
    themes.insert("default".into(), Theme::default());
    Ok(Self {
      themes: themes.into_values().collect(),
      issues,
    })
  }
}
