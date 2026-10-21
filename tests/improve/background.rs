use ainz::{Config, MemoryBackend, improve::automatic::AutomaticStore};
use serde_json::{Value, json};
use tokio::fs;

#[tokio::test]
async fn one_enable_action_makes_an_ordinary_run_launch_a_bounded_background_experiment() {
  let temp = tempfile::tempdir().unwrap();
  let source = temp.path().join("source");
  super::evaluation::repository(&source).await;
  let workspace = source.canonicalize().unwrap();
  let (endpoint, server) = super::evaluation::provider(false).await;
  let config_path = temp.path().join("config.toml");
  let mut config = Config {
    model: "fixture".into(),
    endpoint,
    api_key_env: String::new(),
    provider_retries: 0,
    ..Default::default()
  };
  config.memory.backend = MemoryBackend::Off;
  config.improve.every_runs = 1;
  config.improve.max_seconds = 240;
  fs::write(&config_path, toml::to_string(&config).unwrap())
    .await
    .unwrap();
  let output = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["improve", "on", "--source"])
    .arg(&workspace)
    .args(["--check", "grep -q fixed src/lib.rs"])
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let output = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["ask", "--json", "inspect the observed failure"])
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let data = if cfg!(target_os = "macos") {
    temp.path().join("Library/Application Support")
  } else {
    temp.path().join(".local/share")
  };
  let store = AutomaticStore {
    root: data.join("ainz/improvements/automatic"),
  };
  let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), async {
    loop {
      let state = store.read().unwrap();
      if state.active.is_none()
        && let Some(outcome) = state
          .projects
          .get(&workspace)
          .and_then(|p| p.latest.clone())
      {
        break outcome;
      }
      tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
  })
  .await
  .unwrap();
  assert!(outcome.error.is_none(), "{:?}", outcome.error);
  server.abort();
  let root = outcome.experiment.unwrap();
  assert!(root.join("changes.patch").exists());
  assert!(root.join("skills/proposed/verify-fix/SKILL.md").exists());
  assert_eq!(
    fs::read_to_string(workspace.join("src/lib.rs"))
      .await
      .unwrap(),
    super::evaluation::ORIGINAL
  );
  // cleanup completes after publishing the result, and the worker survives its launching process
  tokio::time::timeout(std::time::Duration::from_secs(5), async {
    while root.join("candidate").exists() {
      tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
  })
  .await
  .unwrap();
  let output = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["improve", "review"])
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  let report = fs::read(root.join("report.json")).await.unwrap();
  let mut altered: Value = serde_json::from_slice(&report).unwrap();
  altered["output"] = json!("changed since review");
  fs::write(
    root.join("report.json"),
    serde_json::to_vec(&altered).unwrap(),
  )
  .await
  .unwrap();
  let rejected = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["improve", "approve"])
    .output()
    .await
    .unwrap();
  assert!(!rejected.status.success());
  assert!(String::from_utf8_lossy(&rejected.stderr).contains("reviewed result changed"));
  fs::write(root.join("report.json"), report).await.unwrap();
  let saved_config = fs::read_to_string(&config_path).await.unwrap();
  let mut changed: Config = toml::from_str(&saved_config).unwrap();
  changed.improve.checks = vec!["true".into()];
  fs::write(&config_path, toml::to_string(&changed).unwrap())
    .await
    .unwrap();
  let rejected = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["improve", "approve"])
    .output()
    .await
    .unwrap();
  assert!(!rejected.status.success());
  assert!(String::from_utf8_lossy(&rejected.stderr).contains("acceptance checks changed"));
  fs::write(&config_path, saved_config).await.unwrap();
  let saved_config = fs::read_to_string(&config_path).await.unwrap();
  let mut changed: Config = toml::from_str(&saved_config).unwrap();
  changed.model = "different-model".into();
  fs::write(&config_path, toml::to_string(&changed).unwrap())
    .await
    .unwrap();
  let rejected = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["improve", "approve"])
    .output()
    .await
    .unwrap();
  assert!(!rejected.status.success());
  assert!(String::from_utf8_lossy(&rejected.stderr).contains("model or runtime settings changed"));
  fs::write(&config_path, saved_config).await.unwrap();
  store
    .update(|state| {
      state
        .projects
        .get_mut(&workspace)
        .unwrap()
        .latest
        .as_mut()
        .unwrap()
        .experiment = Some(temp.path().join("a newer result"));
      Ok(())
    })
    .unwrap();
  let output = super::cli::command(temp.path(), &config_path)
    .arg("--workspace")
    .arg(&workspace)
    .args(["improve", "approve"])
    .output()
    .await
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
  assert_eq!(
    fs::read_to_string(workspace.join("src/lib.rs"))
      .await
      .unwrap(),
    super::evaluation::REPAIRED
  );
  let config_root = if cfg!(target_os = "macos") {
    temp.path().join("Library/Application Support")
  } else {
    temp.path().join(".config")
  };
  assert!(config_root.join("ainz/skills/verify-fix/SKILL.md").exists());
}
