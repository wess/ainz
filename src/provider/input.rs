//! What a process provider is handed on stdin: the conversation as one text prompt, wrapped as
//! a single stream-json user message when the command was told to read that.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::protocol::{Image, Message};

/// The args are what tell the command how to read stdin, so they decide what it is sent.
pub(super) fn reads_json(args: &[String]) -> bool {
  args.iter().any(|arg| arg == "--input-format=stream-json")
    || args
      .windows(2)
      .any(|pair| pair[0] == "--input-format" && pair[1] == "stream-json")
}

/// Where a picture whose file is gone is written out again: the person's own cache, never a
/// temporary folder another account can reach first.
pub(super) fn images() -> PathBuf {
  dirs::cache_dir()
    .unwrap_or_else(std::env::temp_dir)
    .join("ainz/images")
}

pub(super) async fn render(messages: &[&Message], json: bool, images: &Path) -> Result<String> {
  let prompt = transcript(messages, json, images).await?;
  if !json {
    return Ok(prompt);
  }
  let mut content = vec![json!({"type": "text", "text": prompt})];
  content.extend(
    messages
      .iter()
      .flat_map(|message| &message.images)
      .filter_map(|image| image.parts())
      .map(image_block),
  );
  let line = json!({"type": "user", "message": {"role": "user", "content": content}});
  Ok(format!("{line}\n"))
}

fn image_block((media_type, data): (&str, &str)) -> Value {
  json!({
    "type": "image",
    "source": {"type": "base64", "media_type": media_type, "data": data},
  })
}

/// A command that reads stream-json is handed the pictures themselves; one that reads text is
/// handed a file for each, which its own model opens with its own tools.
async fn transcript(messages: &[&Message], json: bool, images: &Path) -> Result<String> {
  let mut prompt = String::new();
  for message in messages {
    prompt.push_str(message.role.label());
    prompt.push_str(":\n");
    if let Some(content) = &message.content {
      prompt.push_str(content);
      prompt.push('\n');
    }
    for call in &message.tool_calls {
      prompt.push_str(&format!("tool call {}: {}\n", call.name, call.arguments));
    }
    for image in &message.images {
      if json && image.parts().is_some() {
        prompt.push_str("[image attached]\n");
        continue;
      }
      match picture_file(image, images).await? {
        Some(path) => prompt.push_str(&format!("[image file: {}]\n", path.display())),
        None => prompt.push_str(&format!("[image: {}]\n", image.url)),
      }
    }
    prompt.push('\n');
  }
  Ok(prompt)
}

/// A file the command can open for a picture: the one it came from while that is still there,
/// otherwise its bytes written out again, since a resumed session outlives the file. Nothing
/// means the picture is a link the command can fetch itself.
async fn picture_file(image: &Image, folder: &Path) -> Result<Option<PathBuf>> {
  if let Some(path) = &image.path
    && tokio::fs::metadata(path)
      .await
      .is_ok_and(|found| found.is_file())
  {
    return Ok(Some(path.clone()));
  }
  let Some((media_type, data)) = image.parts() else {
    return Ok(None);
  };
  let bytes = STANDARD.decode(data).context("decode an attached image")?;
  // named for its contents: the whole transcript goes out every turn, and each picture in it
  // should be one file, not one per turn
  let digest = format!("{:x}", Sha256::digest(&bytes));
  let extension = media_type.strip_prefix("image/").unwrap_or("png");
  let path = folder.join(format!("{}.{extension}", &digest[..24]));
  if tokio::fs::metadata(&path).await.is_err() {
    write(folder, &path, bytes).await?;
  }
  Ok(Some(path))
}

#[cfg(unix)]
async fn write(folder: &Path, path: &Path, bytes: Vec<u8>) -> Result<()> {
  use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
  let (folder, path) = (folder.to_path_buf(), path.to_path_buf());
  tokio::task::spawn_blocking(move || -> Result<()> {
    std::fs::DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(&folder)
      .with_context(|| format!("create {}", folder.display()))?;
    let partial = path.with_extension(format!("{}.part", uuid::Uuid::now_v7().simple()));
    let mut file = std::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .mode(0o600)
      .open(&partial)
      .with_context(|| format!("write {}", partial.display()))?;
    std::io::Write::write_all(&mut file, &bytes)?;
    std::fs::rename(&partial, &path).with_context(|| format!("write {}", path.display()))
  })
  .await?
}

#[cfg(not(unix))]
async fn write(folder: &Path, path: &Path, bytes: Vec<u8>) -> Result<()> {
  tokio::fs::create_dir_all(folder).await?;
  tokio::fs::write(path, bytes).await?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn picture(url: &str, path: Option<PathBuf>) -> Image {
    Image {
      url: url.into(),
      detail: None,
      path,
    }
  }

  async fn text(images: Vec<Image>, folder: &Path) -> String {
    let message = Message::user("what is in this", images);
    render(&[&message], false, folder).await.unwrap()
  }

  #[tokio::test]
  async fn an_attached_file_reaches_the_command_as_its_path() {
    let folder = tempfile::tempdir().unwrap();
    let shot = folder.path().join("shot one.png");
    std::fs::write(&shot, b"x").unwrap();
    let prompt = text(
      vec![picture("data:image/png;base64,eA==", Some(shot.clone()))],
      folder.path(),
    )
    .await;
    assert!(
      prompt.contains(&format!("[image file: {}]", shot.display())),
      "{prompt}"
    );
    assert!(!prompt.contains("base64"), "{prompt}");
  }

  #[tokio::test]
  async fn a_picture_whose_file_is_gone_is_written_out_again_once() {
    let folder = tempfile::tempdir().unwrap();
    let images = folder.path().join("images");
    let gone = folder.path().join("cleared.png");
    let image = picture("data:image/png;base64,eA==", Some(gone));
    let first = text(vec![image.clone()], &images).await;
    let second = text(vec![image], &images).await;
    let written: Vec<PathBuf> = std::fs::read_dir(&images)
      .unwrap()
      .map(|entry| entry.unwrap().path())
      .collect();
    assert_eq!(written.len(), 1, "{written:?}");
    assert_eq!(std::fs::read(&written[0]).unwrap(), b"x");
    assert!(first.contains(&format!("[image file: {}]", written[0].display())));
    assert_eq!(first, second);
  }

  #[tokio::test]
  async fn a_pasted_picture_without_a_file_still_reaches_the_command() {
    let folder = tempfile::tempdir().unwrap();
    let prompt = text(
      vec![picture("data:image/jpeg;base64,eA==", None)],
      folder.path(),
    )
    .await;
    assert!(prompt.contains("[image file: "), "{prompt}");
    assert!(prompt.contains(".jpeg]"), "{prompt}");
  }

  #[tokio::test]
  async fn an_image_the_command_can_fetch_is_passed_along() {
    let folder = tempfile::tempdir().unwrap();
    let prompt = text(
      vec![picture("https://example.test/plot.png", None)],
      folder.path(),
    )
    .await;
    assert!(prompt.contains("[image: https://example.test/plot.png]"));
  }
}
