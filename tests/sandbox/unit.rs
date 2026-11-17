use std::{
  ffi::OsString,
  path::{Path, PathBuf},
  process::Stdio,
};

use super::*;

fn shell(command: &str) -> Vec<OsString> {
  ["sh", "-c", command].map(OsString::from).to_vec()
}

fn arguments(command: &Command) -> Vec<String> {
  command
    .as_std()
    .get_args()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect()
}

#[test]
fn an_unset_policy_is_off() {
  assert_eq!(parse(None).unwrap(), Policy::Off);
  assert_eq!(parse(Some("")).unwrap(), Policy::Off);
  assert_eq!(parse(Some("  ")).unwrap(), Policy::Off);
  assert_eq!(parse(Some("off")).unwrap(), Policy::Off);
}

#[test]
fn a_stated_policy_is_read() {
  assert_eq!(parse(Some("workspace")).unwrap(), Policy::Workspace);
  assert_eq!(parse(Some(" Strict ")).unwrap(), Policy::Strict);
}

#[test]
fn a_policy_nobody_defined_is_refused_rather_than_ignored() {
  for value in ["strcit", "yes", "sandbox", "workspaces"] {
    assert!(parse(Some(value)).is_err(), "{value} should fail");
  }
}

#[test]
fn no_policy_runs_the_command_exactly_as_before() {
  let mut command = Command::new("sh");
  command.args(["-c", "echo hi"]).current_dir("/");
  let command = confine_with(Policy::Off, command, &[]).unwrap();
  assert_eq!(command.as_std().get_program(), "sh");
  assert_eq!(arguments(&command), ["-c", "echo hi"]);
}

#[test]
fn a_command_without_a_working_directory_is_refused_rather_than_guessed() {
  let mut command = Command::new("sh");
  command.args(["-c", "echo hi"]);
  assert!(confine_with(Policy::Workspace, command, &[]).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn a_policy_runs_the_command_under_seatbelt_with_its_environment() {
  let mut command = Command::new("sh");
  command
    .args(["-c", "echo hi"])
    .current_dir("/")
    .env("CARGO_TARGET_DIR", "/elsewhere");
  let confined = confine_with(Policy::Workspace, command, &[]).unwrap();
  assert_eq!(confined.as_std().get_program(), SANDBOX_EXEC);
  let args = arguments(&confined);
  assert_eq!(args[0], "-p");
  assert!(args[1].starts_with("(version 1)"));
  assert!(args[1].contains("(deny file-write*)"));
  assert!(!args[1].contains("(deny network*)"));
  assert_eq!(&args[2..], ["sh", "-c", "echo hi"]);
  assert!(
    confined
      .as_std()
      .get_envs()
      .any(|(key, value)| key == "CARGO_TARGET_DIR" && value.is_some())
  );
  let strict = build(Policy::Strict, Path::new("/"), &shell("echo hi"), &[]).unwrap();
  assert!(arguments(&strict)[1].contains("(deny network*)"));
}

#[cfg(target_os = "linux")]
#[test]
fn a_policy_runs_the_command_under_bubblewrap() {
  if !Path::new(BWRAP).exists() {
    assert!(build(Policy::Workspace, Path::new("/"), &shell("echo hi"), &[]).is_err());
    return;
  }
  let args = |policy| arguments(&build(policy, Path::new("/"), &shell("echo hi"), &[]).unwrap());
  let workspace = args(Policy::Workspace);
  assert!(workspace.windows(3).any(|w| w == ["--ro-bind", "/", "/"]));
  assert!(!workspace.contains(&"--unshare-net".to_string()));
  assert!(workspace.ends_with(&["sh".into(), "-c".into(), "echo hi".into()]));
  assert!(args(Policy::Strict).contains(&"--unshare-net".to_string()));
}

fn scratch(name: &str) -> PathBuf {
  let path = std::env::temp_dir().join(format!("ainz-{name}-{}", uuid::Uuid::now_v7().simple()));
  std::fs::create_dir_all(&path).unwrap();
  std::fs::canonicalize(&path).unwrap()
}

/// The boundary is only worth having if the kernel agrees with it, so this asks the kernel.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn the_boundary_holds_where_it_is_drawn() {
  let confiner = if cfg!(target_os = "macos") {
    SANDBOX_EXEC
  } else {
    BWRAP
  };
  if !Path::new(confiner).exists() {
    return;
  }
  let workspace = scratch("workspace");
  // both probes live under $HOME rather than a temporary directory, which the profile leaves
  // writable on purpose; a probe in there would pass no matter what
  let home = PathBuf::from(std::env::var("HOME").unwrap());
  let outside = home.join(format!(
    ".ainz-sandbox-probe-{}",
    uuid::Uuid::now_v7().simple()
  ));
  let elsewhere = home.join(format!(
    ".ainz-sandbox-handed-{}",
    uuid::Uuid::now_v7().simple()
  ));
  std::fs::create_dir_all(&elsewhere).unwrap();
  let run = |command: String, writable: Vec<PathBuf>| {
    let workspace = workspace.clone();
    async move {
      build(Policy::Workspace, &workspace, &shell(&command), &writable)
        .unwrap()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap()
        .success()
    }
  };

  let inside = workspace.join("allowed.txt");
  assert!(run(format!("echo allowed > {}", inside.display()), vec![]).await);
  assert!(inside.exists());

  assert!(!run(format!("echo denied > {}", outside.display()), vec![]).await);
  assert!(!outside.exists(), "a write to $HOME escaped the sandbox");

  let other = elsewhere.join("handed.txt");
  assert!(!run(format!("echo denied > {}", other.display()), vec![]).await);
  assert!(
    run(
      format!("echo allowed > {}", other.display()),
      vec![elsewhere.clone()]
    )
    .await,
    "a path handed to the profile must be writable"
  );

  drop(std::fs::remove_dir_all(&workspace));
  drop(std::fs::remove_dir_all(&elsewhere));
  drop(std::fs::remove_file(&outside));
}

/// Strict promises a command cannot open a connection, so a listener on this machine is the
/// witness, and `workspace` beside it shows the listener was reachable at all.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn strict_leaves_a_command_no_way_out() {
  let confiner = if cfg!(target_os = "macos") {
    SANDBOX_EXEC
  } else {
    BWRAP
  };
  if !Path::new(confiner).exists() || !Path::new("/usr/bin/curl").exists() {
    return;
  }
  let workspace = scratch("network");
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();
  let command = format!("/usr/bin/curl -s -m 3 http://127.0.0.1:{port}/");
  let reached = |policy| {
    let argv = shell(&command);
    let workspace = workspace.clone();
    let listener = &listener;
    async move {
      let mut child = build(policy, &workspace, &argv, &[])
        .unwrap()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
      let accepted =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept()).await;
      drop(child.wait().await);
      accepted.is_ok()
    }
  };
  assert!(
    reached(Policy::Workspace).await,
    "the listener must be reachable"
  );
  assert!(
    !reached(Policy::Strict).await,
    "a strict command reached the network"
  );
  drop(std::fs::remove_dir_all(&workspace));
}
