use agentx::HeaderCatalog;
use ratatui::{
  style::{Color, Modifier},
  text::Line,
};

#[tokio::test]
async fn discovers_and_parses_safe_ansi_headers() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".agentx/headers");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(
    root.join("neon.ans"),
    b"\x1b[38;2;72;205;214;1mAGENTX\x1b[0m\n  glow",
  )
  .await
  .unwrap();

  let catalog = HeaderCatalog::discover(temp.path()).await.unwrap();
  let header = catalog.get("neon").unwrap();

  assert_eq!(header.width, 6);
  assert_eq!(header.lines.len(), 2);
  assert_eq!(header.lines[0].spans[0].content, "AGENTX");
  assert_eq!(
    header.lines[0].spans[0].style.fg,
    Some(Color::Rgb(72, 205, 214))
  );
  assert!(
    header.lines[0].spans[0]
      .style
      .add_modifier
      .contains(Modifier::BOLD)
  );
}

#[tokio::test]
async fn rejects_terminal_control_sequences() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".agentx/headers");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(root.join("move.ans"), b"hello\x1b[2Jworld")
    .await
    .unwrap();

  let catalog = HeaderCatalog::discover(temp.path()).await.unwrap();

  assert!(catalog.get("move").is_none());
  assert!(
    catalog
      .issues
      .iter()
      .any(|issue| issue.contains("only ANSI SGR"))
  );
}

#[tokio::test]
async fn nearest_header_definition_wins() {
  let temp = tempfile::tempdir().unwrap();
  let workspace = temp.path().join("project/nested");
  let outer = temp.path().join(".agentx/headers");
  let inner = temp.path().join("project/.agentx/headers");
  tokio::fs::create_dir_all(&workspace).await.unwrap();
  tokio::fs::create_dir_all(&outer).await.unwrap();
  tokio::fs::create_dir_all(&inner).await.unwrap();
  tokio::fs::write(outer.join("logo.txt"), "outer")
    .await
    .unwrap();
  tokio::fs::write(inner.join("logo.txt"), "inner")
    .await
    .unwrap();

  let catalog = HeaderCatalog::discover(&workspace).await.unwrap();
  let header = catalog.get("logo").unwrap();

  assert_eq!(header.lines, [Line::raw("inner")]);
}

// the masthead studio writes exactly this shape: half blocks carrying the top pixel as
// foreground and the bottom as background, with a reset where the art is transparent
#[tokio::test]
async fn half_block_pixel_art_keeps_both_pixels_of_a_cell() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".agentx/headers");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(
    root.join("pixels.ans"),
    "\x1b[0m\x1b[38;2;255;0;0;48;2;0;255;0m\u{2580}\x1b[0m\x1b[38;2;0;0;255m\u{2588}\x1b[0m\x1b[38;2;255;255;0m\u{2584}\x1b[0m\n",
  )
  .await
  .unwrap();

  let catalog = HeaderCatalog::discover(temp.path()).await.unwrap();
  let header = catalog.get("pixels").unwrap();
  let spans = &header.lines[0].spans;

  assert_eq!(header.width, 3);
  assert_eq!(spans[0].content, "\u{2580}");
  assert_eq!(spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
  assert_eq!(spans[0].style.bg, Some(Color::Rgb(0, 255, 0)));
  assert_eq!(spans[1].content, "\u{2588}");
  assert_eq!(spans[1].style.fg, Some(Color::Rgb(0, 0, 255)));
  assert_eq!(spans[1].style.bg, None);
  assert_eq!(spans[2].content, "\u{2584}");
  assert_eq!(spans[2].style.fg, Some(Color::Rgb(255, 255, 0)));
}

#[tokio::test]
async fn only_ansi_and_text_extensions_are_loaded() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".agentx/headers");
  tokio::fs::create_dir_all(&root).await.unwrap();
  let art = "\x1b[38;2;1;2;3m\u{2588}\x1b[0m\n";
  for name in ["keep.ans", "keep2.ansi", "keep3.txt"] {
    tokio::fs::write(root.join(name), art).await.unwrap();
  }
  for name in ["skip.json", "skip.png", "skip"] {
    tokio::fs::write(root.join(name), art).await.unwrap();
  }

  let catalog = HeaderCatalog::discover(temp.path()).await.unwrap();
  let mut names: Vec<_> = catalog.headers.iter().map(|art| art.name.clone()).collect();
  names.sort();

  assert_eq!(names, ["keep", "keep2", "keep3"]);
  assert!(catalog.issues.is_empty());
}
