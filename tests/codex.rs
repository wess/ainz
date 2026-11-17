//! The real `codex`, handed a pasted picture. Ignored by default: it needs the CLI signed in
//! and spends a request. `cargo test --test codex -- --ignored` runs it.

use ainz::{
  EventSink, PermissionMode, ProcessProvider,
  protocol::{Image, Message},
  provider::ChatProvider,
};

/// The model codex is configured with on this machine, which the preset passes by name.
fn configured() -> String {
  let home = std::env::var("HOME").unwrap();
  let config = std::fs::read_to_string(format!("{home}/.codex/config.toml")).unwrap_or_default();
  config
    .lines()
    .find_map(|line| line.strip_prefix("model = "))
    .map(|model| model.trim().trim_matches('"').to_string())
    .unwrap_or_else(|| "gpt-5".into())
}

#[tokio::test]
#[ignore]
async fn a_pasted_picture_reaches_codex_as_a_file_it_can_open() {
  let workspace = tempfile::tempdir().unwrap();
  let images = tempfile::tempdir().unwrap();
  let preset = ainz::config::ProviderConfig::codex();
  let provider = ProcessProvider::new(
    "codex".into(),
    preset.args,
    configured(),
    workspace.path().into(),
    PermissionMode::ReadOnly,
    preset.output,
  )
  .with_images(images.path().into());
  // pasted from the clipboard: no file to point at, so it is written out for the command
  let mut picture = Image::from_path(std::path::Path::new("fixtures/images/red.png"))
    .await
    .unwrap();
  picture.path = None;
  let prompt = Message::user(
    "What single colour fills the attached image? Answer with one lowercase word.",
    vec![picture],
  );

  let reply = provider
    .complete(&[&prompt], &[], &EventSink::default())
    .await
    .unwrap();

  let answer = reply.message.content.unwrap_or_default().to_lowercase();
  assert!(answer.contains("red"), "{answer}");
}
