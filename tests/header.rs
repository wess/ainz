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
