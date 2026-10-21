use std::path::Path;

use ainz::{Config, MemoryBackend, PermissionMode};
use serde_json::Value;
use tokio::{fs, process::Command};

pub(super) fn command(home: &Path, config: &Path) -> Command {
  let mut command = Command::new(env!("CARGO_BIN_EXE_ainz"));
  // version-manager shims depend on the real home; use the underlying tools in this fixture
  let paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
    .filter(|path| !path.ends_with("shims"))
    .collect();
  command
    .env("PATH", std::env::join_paths(paths).unwrap())
    .env("HOME", home)
    .env("XDG_CONFIG_HOME", home.join(".config"))
    .env("XDG_DATA_HOME", home.join(".local/share"))
    .env("AINZ_CONFIG", config)
    .env_remove("AINZ_PROVIDER")
    .env_remove("AINZ_MODEL")
    .env_remove("AINZ_ENDPOINT")
    .env_remove("AINZ_MEMORY")
    .env_remove("AINZ_SYNAPSE")
    .env(
      "RUSTUP_HOME",
      std::env::var_os("RUSTUP_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
          std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".rustup")
        }),
    )
    .env(
      "CARGO_HOME",
      std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
          std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo")
        }),
    );
  command
}

#[tokio::test]
async fn improve_runs_the_real_loop_checks_its_patch_and_keeps_learning_uninstalled() {
  exercise(true).await;
}

#[tokio::test]
async fn a_step_limited_improvement_cannot_pass_even_when_its_candidate_checks_do() {
  exercise(false).await;
}

async fn exercise(complete: bool) {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  super::evaluation::repository(&source).await;
  let (endpoint, server) = super::evaluation::provider(false).await;
  let config_path = temp.path().join("config.toml");
  let mut config = Config {
    model: "fixture".into(),
    endpoint,
    api_key_env: String::new(),
    permissions: PermissionMode::Auto,
    provider_retries: 0,
    max_steps: if complete { 4 } else { 1 },
    ..Default::default()
  };
  config.memory.backend = MemoryBackend::Off;
  fs::write(&config_path, toml::to_string(&config).unwrap())
    .await
    .unwrap();
  let output = tokio::time::timeout(
    std::time::Duration::from_secs(240),
    command(temp.path(), &config_path)
      .arg("--workspace")
      .arg(&source)
      .args([
        "improve",
        "--json",
        "--focus",
        "the value should be fixed",
        "--check",
        "grep -q fixed src/lib.rs",
        "--out",
      ])
      .arg(temp.path().join("experiments"))
      .output(),
  )
  .await
  .unwrap()
  .unwrap();
  assert_eq!(
    output.status.success(),
    complete,
    "{}\n{}",
    String::from_utf8_lossy(&output.stderr),
    String::from_utf8_lossy(&output.stdout)
  );
  server.abort();
  let final_event: Value = String::from_utf8(output.stdout)
    .unwrap()
    .lines()
    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    .find(|event| event["type"] == "improvement")
    .unwrap();
  assert_eq!(final_event["report"]["checks_passed"], complete);
  assert_eq!(final_event["report"]["baseline"][0]["code"], 1);
  assert_eq!(final_event["report"]["candidate"][0]["code"], 0);
  assert!(
    final_event["report"]["usage"]["input_tokens"]
      .as_u64()
      .unwrap()
      >= 6
  );
  assert_eq!(
    fs::read_to_string(source.join("src/lib.rs")).await.unwrap(),
    super::evaluation::ORIGINAL
  );
  let experiment = Path::new(final_event["path"].as_str().unwrap());
  assert!(
    experiment
      .join("skills/proposed/verify-fix/SKILL.md")
      .exists()
  );
  let config_root = if cfg!(target_os = "macos") {
    temp.path().join("Library/Application Support")
  } else {
    temp.path().join(".config")
  };
  let installed = config_root.join("ainz/skills/verify-fix/SKILL.md");
  assert!(!installed.exists());
  if !complete {
    assert!(
      final_event["report"]["agent_error"]
        .as_str()
        .unwrap()
        .contains("paused")
    );
    return;
  }
  let output = command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&source)
    .args(["skills", "approve", "verify-fix", "--from"])
    .arg(experiment.join("skills"))
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(installed.exists());
  assert!(
    experiment
      .join("skills/proposed/verify-fix/SKILL.md")
      .exists()
  );
}
