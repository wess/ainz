#![cfg(feature = "cli")]

use std::{
  collections::HashMap,
  path::{Path, PathBuf},
  process::Command,
  sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  },
};

use ainz::update::{
  Channel, State, Updater, archive, background, install, latest, load, newer, save, target,
  unmarked,
};
use sha2::{Digest, Sha256};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
};

type Routes = HashMap<String, (String, Vec<u8>)>;

// a release page: each path answers with its status line and body, and every request is counted
async fn serve(routes: Routes) -> (String, Arc<AtomicUsize>) {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let hits = Arc::new(AtomicUsize::new(0));
  let counter = hits.clone();
  tokio::spawn(async move {
    while let Ok((mut stream, _)) = listener.accept().await {
      counter.fetch_add(1, Ordering::SeqCst);
      let routes = routes.clone();
      tokio::spawn(async move {
        let mut buffer = vec![0; 4096];
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let mut words = request.split_whitespace();
        let method = words.next().unwrap_or_default();
        let path = words.next().unwrap_or_default();
        let (head, body) = routes
          .get(path)
          .cloned()
          .unwrap_or(("404 Not Found".into(), Vec::new()));
        let answer = format!(
          "HTTP/1.1 {head}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
          body.len()
        );
        drop(stream.write_all(answer.as_bytes()).await);
        if method != "HEAD" {
          drop(stream.write_all(&body).await);
        }
      });
    }
  });
  (base, hits)
}

fn script(directory: &Path, version: &str) -> Vec<u8> {
  let staged = directory.join("staged");
  std::fs::create_dir_all(&staged).unwrap();
  let binary = staged.join("ainz");
  std::fs::write(&binary, format!("#!/bin/sh\necho 'ainz {version}'\n")).unwrap();
  std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
  let packed = directory.join("packed.tar.gz");
  let status = Command::new("tar")
    .arg("-czf")
    .arg(&packed)
    .arg("-C")
    .arg(&staged)
    .arg("ainz")
    .status()
    .unwrap();
  assert!(status.success());
  std::fs::read(packed).unwrap()
}

fn release(version: &str, packed: Vec<u8>, digest: String) -> Routes {
  let name = archive(version, target().unwrap());
  let file = format!("/download/v{version}/{name}");
  HashMap::from([
    (
      "/latest".to_string(),
      (
        format!("302 Found\r\nlocation: /tag/v{version}"),
        Vec::new(),
      ),
    ),
    (format!("/tag/v{version}"), ("200 OK".into(), Vec::new())),
    (file.clone(), ("200 OK".into(), packed)),
    (
      format!("{file}.sha256"),
      ("200 OK".into(), format!("{digest}  {name}\n").into_bytes()),
    ),
  ])
}

fn good(directory: &Path, version: &str) -> Routes {
  let packed = script(directory, version);
  let digest = format!("{:x}", Sha256::digest(&packed));
  release(version, packed, digest)
}

fn installed(directory: &Path, name: &str) -> PathBuf {
  let exe = directory.join(name);
  std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
  std::fs::write(&exe, "old").unwrap();
  exe
}

fn entries(directory: &Path) -> Vec<String> {
  std::fs::read_dir(directory)
    .unwrap()
    .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
    .collect()
}

#[test]
fn versions_compare_as_numbers_and_strange_ones_never_win() {
  assert!(newer("0.10.0", "0.9.9"));
  assert!(newer("1.0.0", "0.99.99"));
  assert!(!newer("0.15.2", "0.15.2"));
  assert!(!newer("0.15.1", "0.15.2"));
  assert!(!newer("latest", "0.15.2"));
  assert!(!newer("1.0.0-rc1", "0.15.2"));
  assert!(!newer("1.0", "0.15.2"));
}

#[test]
fn the_install_path_says_who_owns_the_file() {
  let of = |path: &str| Channel::of(Path::new(path));
  assert_eq!(
    of("/opt/homebrew/Cellar/ainz/0.15.1/bin/ainz"),
    Channel::Homebrew
  );
  assert_eq!(
    of("/home/linuxbrew/.linuxbrew/Cellar/ainz/0.15.1/bin/ainz"),
    Channel::Homebrew
  );
  assert_eq!(of("/Users/me/.cargo/bin/ainz"), Channel::Cargo);
  assert_eq!(of("/Users/me/ainz/target/release/ainz"), Channel::Cargo);
  assert_eq!(of("/Users/me/.local/bin/ainz"), Channel::Standalone);
  assert_eq!(of("/usr/local/bin/ainz"), Channel::Standalone);
}

#[test]
fn a_replaced_executable_is_found_at_its_real_path() {
  assert_eq!(
    unmarked(PathBuf::from("/home/me/.local/bin/ainz (deleted)")),
    PathBuf::from("/home/me/.local/bin/ainz")
  );
  assert_eq!(
    unmarked(PathBuf::from("/home/me/.local/bin/ainz")),
    PathBuf::from("/home/me/.local/bin/ainz")
  );
}

#[tokio::test]
async fn the_latest_version_is_read_from_the_redirect() {
  let dir = tempfile::tempdir().unwrap();
  let (base, _) = serve(good(dir.path(), "9.9.9")).await;
  assert_eq!(latest(&base).await.unwrap(), "9.9.9");
}

#[tokio::test]
async fn a_tag_that_is_not_a_version_is_refused() {
  let routes = HashMap::from([
    (
      "/latest".to_string(),
      (
        "302 Found\r\nlocation: /tag/nightly".to_string(),
        Vec::new(),
      ),
    ),
    (
      "/tag/nightly".to_string(),
      ("200 OK".to_string(), Vec::new()),
    ),
  ]);
  let (base, _) = serve(routes).await;
  assert!(latest(&base).await.is_err());
}

#[tokio::test]
async fn install_swaps_the_binary_and_leaves_nothing_behind() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("bin"), "ainz");
  let (base, _) = serve(good(dir.path(), "9.9.9")).await;

  install(&base, "9.9.9", &exe).await.unwrap();

  let output = Command::new(&exe).arg("--version").output().unwrap();
  assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ainz 9.9.9");
  assert_eq!(entries(exe.parent().unwrap()), ["ainz"]);
}

#[tokio::test]
async fn a_bad_checksum_leaves_the_binary_alone() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("bin"), "ainz");
  let packed = script(dir.path(), "9.9.9");
  let (base, _) = serve(release("9.9.9", packed, "0".repeat(64))).await;

  let error = install(&base, "9.9.9", &exe).await.unwrap_err();

  assert!(format!("{error:#}").contains("checksum"), "{error:#}");
  assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
  assert_eq!(entries(exe.parent().unwrap()), ["ainz"]);
}

#[tokio::test]
async fn a_binary_that_reports_the_wrong_version_is_not_installed() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("bin"), "ainz");
  let packed = script(dir.path(), "1.2.3");
  let digest = format!("{:x}", Sha256::digest(&packed));
  let (base, _) = serve(release("9.9.9", packed, digest)).await;

  let error = install(&base, "9.9.9", &exe).await.unwrap_err();

  assert!(
    format!("{error:#}").contains("expected ainz 9.9.9"),
    "{error:#}"
  );
  assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
  assert_eq!(entries(exe.parent().unwrap()), ["ainz"]);
}

fn updater(base: String, directory: &Path, exe: PathBuf) -> Updater {
  Updater {
    current: "0.15.2".into(),
    base,
    state: directory.join("state/update.json"),
    exe,
  }
}

#[tokio::test]
async fn a_standalone_install_updates_itself_and_says_so() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("bin"), "ainz");
  let (base, _) = serve(good(dir.path(), "9.9.9")).await;

  let notice = background(&updater(base, dir.path(), exe.clone()), 1_000_000).await;

  assert_eq!(
    notice.as_deref(),
    Some("updated to ainz 9.9.9; restart to use it")
  );
  let output = Command::new(&exe).arg("--version").output().unwrap();
  assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ainz 9.9.9");
}

#[tokio::test]
async fn homebrew_is_told_rather_than_overwritten() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("Cellar/ainz/0.15.2/bin"), "ainz");
  let (base, _) = serve(good(dir.path(), "9.9.9")).await;

  let notice = background(&updater(base, dir.path(), exe.clone()), 1_000_000).await;

  assert_eq!(
    notice.as_deref(),
    Some("ainz 9.9.9 is available; run ainz update")
  );
  assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
}

#[tokio::test]
async fn a_recent_check_keeps_the_launch_off_the_network() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("Cellar/ainz/0.15.2/bin"), "ainz");
  let (base, hits) = serve(good(dir.path(), "9.9.9")).await;
  let updater = updater(base, dir.path(), exe);
  let known = State {
    checked_at: 1_000_000,
    latest: Some("9.9.9".into()),
  };
  save(&updater.state, &known).await.unwrap();

  let notice = background(&updater, 1_000_060).await;

  // the notice still comes from what the last check learned
  assert!(notice.is_some());
  assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn being_current_is_silent_and_recorded() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("bin"), "ainz");
  let (base, _) = serve(good(dir.path(), "0.15.2")).await;
  let updater = updater(base, dir.path(), exe.clone());

  assert_eq!(background(&updater, 1_000_000).await, None);

  assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
  assert_eq!(load(&updater.state).await.checked_at, 1_000_000);
}

#[tokio::test]
async fn an_unreachable_release_page_is_not_news() {
  let dir = tempfile::tempdir().unwrap();
  let exe = installed(&dir.path().join("bin"), "ainz");
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  drop(listener);
  let updater = updater(base, dir.path(), exe);

  assert_eq!(background(&updater, 1_000_000).await, None);
  assert_eq!(load(&updater.state).await, State::default());
}
