#![cfg(feature = "cli")]

use ainz::theme::{Theme, ThemeCatalog};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};

#[test]
fn parses_partial_palette_and_rejects_invalid_files() {
  let theme = Theme::parse("paper", "[colors]\ntext = '#123456'\n").unwrap();
  assert_eq!(theme.color("text"), Some(Color::Rgb(18, 52, 86)));
  assert_eq!(theme.color("background"), Some(Color::Reset));
  for source in [
    "[colors]\ntext = '#123'",
    "[colors]\ntext = 'default'",
    "[colors]\nbackground = '#gg0000'",
    "[colors]\nunknown = '#ffffff'",
    "[colors]\ntext = '#ffffff'\ntext = '#000000'",
    "[colors]\n[script]\ncommand = 'something'",
  ] {
    assert!(Theme::parse("bad", source).is_err(), "{source}");
  }
}

#[test]
fn maps_chat_roles_without_recoloring_artwork() {
  let theme = Theme::parse(
    "custom",
    "[colors]\nbackground = '#010203'\ntext = '#abcdef'\nbar = '#102030'\n\
     bar_text = '#aabbcc'\nborder = '#112233'\ninfo = '#445566'",
  )
  .unwrap();
  let area = Rect::new(3, 4, 4, 2);
  let mut buffer = Buffer::empty(area);
  buffer[(3, 4)].set_symbol("x");
  buffer[(4, 4)]
    .set_fg(Color::White)
    .set_bg(Color::Rgb(24, 66, 128));
  buffer[(5, 4)].set_fg(Color::Rgb(24, 66, 128));
  buffer[(6, 4)].set_fg(Color::Rgb(72, 205, 214));
  buffer[(3, 5)]
    .set_fg(Color::Rgb(72, 205, 214))
    .set_bg(Color::Rgb(24, 66, 128));
  let art = buffer[(3, 5)].clone();
  theme.apply(&mut buffer, Some(Rect::new(3, 5, 4, 1)));
  assert_eq!(buffer[(3, 4)].fg, Color::Rgb(171, 205, 239));
  assert_eq!(buffer[(3, 4)].bg, Color::Rgb(1, 2, 3));
  assert_eq!(buffer[(4, 4)].fg, Color::Rgb(170, 187, 204));
  assert_eq!(buffer[(4, 4)].bg, Color::Rgb(16, 32, 48));
  assert_eq!(buffer[(5, 4)].fg, Color::Rgb(17, 34, 51));
  assert_eq!(buffer[(6, 4)].fg, Color::Rgb(68, 85, 102));
  assert_eq!(buffer[(3, 5)], art);
  assert_eq!(buffer[(4, 5)].bg, Color::Rgb(1, 2, 3));
  assert_eq!(buffer[(4, 5)].fg, Color::Rgb(171, 205, 239));
  let expected = buffer.clone();
  Theme::default().apply(&mut buffer, None);
  assert_eq!(buffer, expected);
}

#[tokio::test]
async fn nearest_theme_wins_and_bad_files_do_not_hide_valid_themes() {
  let temp = tempfile::tempdir().unwrap();
  let outer = temp.path().join(".ainz/themes");
  let project = temp.path().join("project");
  let inner = project.join(".ainz/themes");
  std::fs::create_dir_all(&outer).unwrap();
  std::fs::create_dir_all(&inner).unwrap();
  std::fs::write(outer.join("shared.toml"), "[colors]\ntext = '#000000'").unwrap();
  std::fs::write(inner.join("shared.toml"), "[colors]\ntext = '#ffffff'").unwrap();
  std::fs::write(inner.join("broken.toml"), "[colors]\ntext = 'no'").unwrap();
  std::fs::write(inner.join("huge.toml"), " ".repeat(16385)).unwrap();
  std::fs::write(inner.join("default.toml"), "[colors]").unwrap();
  #[cfg(unix)]
  std::os::unix::fs::symlink(inner.join("shared.toml"), inner.join("linked.toml")).unwrap();
  let catalog = ThemeCatalog::discover(&project).await.unwrap();
  assert_eq!(
    catalog.get("shared").unwrap().color("text"),
    Some(Color::Rgb(255, 255, 255))
  );
  assert_eq!(
    catalog.get("shared").unwrap().path,
    inner.join("shared.toml")
  );
  assert!(catalog.get("broken").is_none());
  assert!(catalog.get("huge").is_none());
  assert!(catalog.get("linked").is_none());
  assert!(catalog.get("default").unwrap().path.as_os_str().is_empty());
  assert!(
    catalog
      .issues
      .iter()
      .any(|issue| issue.contains("theme exceeds 16 KiB"))
  );
  assert!(
    catalog
      .issues
      .iter()
      .any(|issue| issue.contains("invalid or reserved"))
  );
}
