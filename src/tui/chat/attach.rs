//! Pictures on the prompt: a dragged file or the clipboard's image becomes an `[Image #N]` in
//! the line, and a prompt carries whichever of those the person left in it.

use std::path::{Path, PathBuf};

const KINDS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const OPEN: &str = "[Image #";

pub(super) fn token(number: usize) -> String {
  format!("{OPEN}{number}]")
}

/// Where a placeholder that ends at `cursor` starts, so one backspace takes all of it.
pub(super) fn token_before(text: &str, cursor: usize) -> Option<usize> {
  let before = text.get(..cursor)?.strip_suffix(']')?;
  let start = before.rfind(OPEN)?;
  let digits = &before[start + OPEN.len()..];
  (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())).then_some(start)
}

/// The pictures whose placeholder is still in the prompt, in the order they were attached.
pub(super) fn kept(prompt: &str, attached: Vec<(usize, PathBuf)>) -> Vec<PathBuf> {
  attached
    .into_iter()
    .filter(|(number, _)| prompt.contains(&token(*number)))
    .map(|(_, path)| path)
    .collect()
}

/// The pictures a paste names, when it is nothing but paths to them. Dragging files into a
/// terminal pastes their paths, quoted or backslash-escaped depending on the terminal, and the
/// thing somebody meant by that is the pictures.
pub(super) fn paths(text: &str) -> Vec<PathBuf> {
  let text = text.trim();
  if text.is_empty() || text.contains('\n') {
    return Vec::new();
  }
  // a path with bare spaces in it, the way a file manager copies one
  if let Some(path) = picture(text.trim_matches(['\'', '"'])) {
    return vec![path];
  }
  let words = split(text);
  let pictures: Vec<PathBuf> = words.iter().filter_map(|word| picture(word)).collect();
  match pictures.len() == words.len() {
    true => pictures,
    false => Vec::new(),
  }
}

/// Words the way a shell reads them, which is how a terminal escapes the paths it pastes.
fn split(text: &str) -> Vec<String> {
  let mut words = Vec::new();
  let mut word = String::new();
  let mut quote = None;
  let mut characters = text.chars();
  while let Some(character) = characters.next() {
    match (quote, character) {
      (Some(open), _) if character == open => quote = None,
      (Some(_), _) => word.push(character),
      (None, '\\') => word.extend(characters.next()),
      (None, '\'' | '"') => quote = Some(character),
      (None, _) if character.is_whitespace() => {
        if !word.is_empty() {
          words.push(std::mem::take(&mut word));
        }
      }
      (None, _) => word.push(character),
    }
  }
  if !word.is_empty() {
    words.push(word);
  }
  words
}

fn picture(text: &str) -> Option<PathBuf> {
  let bare = text.strip_prefix("file://").unwrap_or(text);
  // a file URL encodes a space; nothing else in one is worth decoding
  let path = PathBuf::from(bare.replace("%20", " "));
  let known = path
    .extension()
    .and_then(|extension| extension.to_str())
    .is_some_and(|extension| KINDS.contains(&extension.to_ascii_lowercase().as_str()));
  (known && path.is_file()).then_some(path)
}

/// Where clipboard pictures are written: the person's own cache, never a temporary folder
/// another account on the machine can reach first.
fn folder() -> PathBuf {
  dirs::cache_dir()
    .unwrap_or_else(std::env::temp_dir)
    .join("ainz/clipboard")
}

/// The clipboard's picture, as a file. It reads a copied file's path when that is what the
/// clipboard holds, and otherwise writes the image out.
pub(super) async fn clipboard() -> Result<PathBuf, String> {
  let folder = folder();
  private(&folder).await.map_err(|error| error.to_string())?;
  let target = folder.join(format!("{}.png", uuid::Uuid::now_v7()));
  let path = tokio::time::timeout(std::time::Duration::from_secs(5), read(&target))
    .await
    .map_err(|_| "the clipboard did not respond".to_string())??;
  picture(&path.to_string_lossy()).ok_or_else(|| "the clipboard holds a file, not a picture".into())
}

#[cfg(unix)]
async fn private(folder: &Path) -> std::io::Result<()> {
  tokio::fs::DirBuilder::new()
    .recursive(true)
    .mode(0o700)
    .create(folder)
    .await
}

#[cfg(not(unix))]
async fn private(folder: &Path) -> std::io::Result<()> {
  tokio::fs::create_dir_all(folder).await
}

/// A copied file comes first: copying one in Finder puts its name on the clipboard along with
/// its icon, and the icon is not what anybody meant.
#[cfg(target_os = "macos")]
const SCRIPT: &[&str] = &[
  "on run argv",
  "try",
  "return POSIX path of (the clipboard as «class furl»)",
  "end try",
  "set png to (the clipboard as «class PNGf»)",
  "set handle to open for access (POSIX file (item 1 of argv)) with write permission",
  "try",
  "set eof handle to 0",
  "write png to handle",
  "on error message",
  "close access handle",
  "error message",
  "end try",
  "close access handle",
  "return item 1 of argv",
  "end run",
];

#[cfg(target_os = "macos")]
async fn read(target: &Path) -> Result<PathBuf, String> {
  let mut command = tokio::process::Command::new("/usr/bin/osascript");
  for line in SCRIPT {
    command.args(["-e", line]);
  }
  let output = command
    .arg(target)
    .stdin(std::process::Stdio::null())
    .kill_on_drop(true)
    .output()
    .await
    .map_err(|error| error.to_string())?;
  if !output.status.success() {
    return Err("the clipboard holds no image".into());
  }
  Ok(PathBuf::from(
    String::from_utf8_lossy(&output.stdout).trim(),
  ))
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn read(target: &Path) -> Result<PathBuf, String> {
  let readers: [(&str, &[&str]); 2] = [
    ("wl-paste", &["--no-newline", "--type", "image/png"]),
    (
      "xclip",
      &["-selection", "clipboard", "-target", "image/png", "-out"],
    ),
  ];
  for (program, args) in readers {
    let Ok(output) = tokio::process::Command::new(program)
      .args(args)
      .stdin(std::process::Stdio::null())
      .kill_on_drop(true)
      .output()
      .await
    else {
      continue;
    };
    if output.status.success() && output.stdout.starts_with(b"\x89PNG") {
      use std::os::unix::fs::OpenOptionsExt;
      let target = target.to_path_buf();
      let bytes = output.stdout;
      return tokio::task::spawn_blocking(move || {
        use std::io::Write;
        std::fs::OpenOptions::new()
          .write(true)
          .create_new(true)
          .mode(0o600)
          .open(&target)
          .and_then(|mut file| file.write_all(&bytes))
          .map(|()| target)
      })
      .await
      .map_err(|error| error.to_string())?
      .map_err(|error| error.to_string());
    }
  }
  Err("the clipboard holds no image, or neither wl-paste nor xclip is installed".into())
}

#[cfg(not(unix))]
async fn read(_target: &Path) -> Result<PathBuf, String> {
  Err("pasting an image needs macOS or Linux; drag the file in instead".into())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"x").unwrap();
    path
  }

  #[test]
  fn a_dragged_picture_is_recognised_by_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let png = file(dir.path(), "shot.png");
    let shown = png.display().to_string();

    assert_eq!(paths(&shown), vec![png.clone()]);
    // the shapes the common terminals send when a file is dragged in
    assert_eq!(paths(&format!("'{shown}'")), vec![png.clone()]);
    assert_eq!(paths(&format!("\"{shown}\"")), vec![png.clone()]);
    assert_eq!(paths(&format!("file://{shown}")), vec![png.clone()]);
    assert_eq!(paths(&format!("{shown} ")), vec![png]);
  }

  #[test]
  fn a_space_in_the_name_survives_every_way_a_terminal_sends_it() {
    let dir = tempfile::tempdir().unwrap();
    let png = file(dir.path(), "Shot 2026-10-30 at 5.48.51 PM.png");
    let shown = png.display().to_string();

    assert_eq!(paths(&shown), vec![png.clone()]);
    assert_eq!(paths(&shown.replace(' ', "%20")), vec![png.clone()]);
    assert_eq!(paths(&shown.replace(' ', "\\ ")), vec![png.clone()]);
    assert_eq!(paths(&format!("'{shown}'")), vec![png]);
  }

  #[test]
  fn several_dragged_pictures_are_all_attached() {
    let dir = tempfile::tempdir().unwrap();
    let first = file(dir.path(), "one shot.png");
    let second = file(dir.path(), "two.jpg");
    let pasted = format!(
      "{} {}",
      first.display().to_string().replace(' ', "\\ "),
      second.display()
    );
    assert_eq!(paths(&pasted), vec![first, second]);
  }

  #[test]
  fn ordinary_pasted_text_is_text() {
    assert!(paths("just some words").is_empty());
    assert!(paths("").is_empty());
    assert!(paths("/tmp/a.png\nand more").is_empty());
    assert!(paths("/nowhere/at/all/missing.png").is_empty());
  }

  #[test]
  fn a_path_among_words_is_text_the_person_is_writing() {
    let dir = tempfile::tempdir().unwrap();
    let png = file(dir.path(), "shot.png");
    assert!(paths(&format!("look at {}", png.display())).is_empty());
  }

  #[test]
  fn a_file_that_is_not_a_picture_is_not_one() {
    let dir = tempfile::tempdir().unwrap();
    let notes = file(dir.path(), "notes.md");
    assert!(paths(&notes.display().to_string()).is_empty());
  }

  #[test]
  fn a_prompt_carries_only_the_pictures_still_named_in_it() {
    let attached = vec![
      (1, PathBuf::from("one")),
      (2, PathBuf::from("two")),
      (10, PathBuf::from("ten")),
    ];
    let kept = kept("compare [Image #10] with [Image #2]", attached);
    assert_eq!(kept, [PathBuf::from("two"), PathBuf::from("ten")]);
  }

  /// A one-pixel PNG, enough for the clipboard to take it as a picture.
  const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
  ];

  /// What was on the clipboard as text, so a test that borrows it can put it back.
  #[cfg(target_os = "macos")]
  fn borrow() -> Vec<u8> {
    std::process::Command::new("pbpaste")
      .output()
      .map(|output| output.stdout)
      .unwrap_or_default()
  }

  #[cfg(target_os = "macos")]
  fn give_back(text: Vec<u8>) {
    use std::io::Write;
    let mut copy = std::process::Command::new("pbcopy")
      .stdin(std::process::Stdio::piped())
      .spawn()
      .unwrap();
    copy.stdin.take().unwrap().write_all(&text).unwrap();
    copy.wait().unwrap();
  }

  #[cfg(target_os = "macos")]
  fn copy(script: &str) {
    let status = std::process::Command::new("/usr/bin/osascript")
      .args(["-e", script])
      .status()
      .unwrap();
    assert!(status.success());
  }

  /// The real clipboard, holding an image and then a copied file. Ignored: it replaces what is
  /// on the clipboard while it runs, then puts the text that was there back.
  #[cfg(target_os = "macos")]
  #[tokio::test]
  #[ignore]
  async fn the_clipboard_gives_up_a_picture_or_a_copied_file() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("probe.png");
    std::fs::write(&png, PNG).unwrap();
    let saved = borrow();
    copy(&format!(
      "set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)",
      png.display()
    ));
    let pasted = clipboard().await;
    copy(&format!(
      "set the clipboard to (POSIX file \"{}\")",
      png.display()
    ));
    let copied = clipboard().await;
    give_back(saved);

    let pasted = pasted.unwrap();
    assert!(std::fs::read(&pasted).unwrap().starts_with(b"\x89PNG"));
    assert_ne!(
      pasted, png,
      "the picture is written out, not the source file"
    );
    let _ = std::fs::remove_file(&pasted);
    assert_eq!(
      std::fs::canonicalize(copied.unwrap()).unwrap(),
      std::fs::canonicalize(&png).unwrap()
    );
  }

  /// The X clipboard through `xclip`. Ignored: it needs a display and xclip, which the Linux
  /// check provides under Xvfb.
  #[cfg(all(unix, not(target_os = "macos")))]
  #[tokio::test]
  #[ignore]
  async fn the_clipboard_gives_up_a_picture() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("probe.png");
    std::fs::write(&png, PNG).unwrap();
    let status = std::process::Command::new("xclip")
      .args(["-selection", "clipboard", "-target", "image/png", "-in"])
      .arg(&png)
      .status()
      .unwrap();
    assert!(status.success());
    let pasted = clipboard().await.unwrap();
    assert_eq!(std::fs::read(&pasted).unwrap(), PNG);
  }

  #[test]
  fn backspace_finds_a_placeholder_only_when_the_caret_is_at_its_end() {
    let text = "see [Image #12] here";
    let end = "see [Image #12]".len();
    assert_eq!(token_before(text, end), Some(4));
    assert_eq!(token_before(text, end - 1), None);
    assert_eq!(token_before(text, text.len()), None);
    assert_eq!(token_before("a [Image #] b]", "a [Image #]".len()), None);
  }
}
