use ainz::{Config, MemoryBackend, ProcessOutput, ProviderConfig};

#[tokio::test]
async fn provider_profiles_round_trip() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  let mut config = Config::default();
  let mut profile = ProviderConfig::process(
    "runner",
    vec!["--model".into(), "{model}".into()],
    ProcessOutput::Text,
  );
  profile.models.push("small".into());
  config.providers.insert("local".into(), profile);
  config.provider = Some("local".into());
  config.model = "small".into();
  config.ui.roster_visible = false;
  config.ui.header = "neon".into();
  config.save_to(&path).await.unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert_eq!(loaded.provider.as_deref(), Some("local"));
  assert_eq!(loaded.model, "small");
  assert_eq!(loaded.providers["local"].command.as_deref(), Some("runner"));
  assert_eq!(loaded.providers["local"].models, ["small"]);
  assert!(!loaded.ui.roster_visible);
  assert_eq!(loaded.ui.header, "neon");
}

#[tokio::test]
async fn legacy_endpoint_config_still_resolves() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(
    &path,
    "endpoint = \"http://127.0.0.1:9999/v1\"\nmodel = \"legacy\"\n",
  )
  .await
  .unwrap();

  let loaded = Config::load_from(&path).await.unwrap();
  let provider = loaded.active_provider().unwrap();

  assert_eq!(
    provider.endpoint.as_deref(),
    Some("http://127.0.0.1:9999/v1")
  );
  assert_eq!(loaded.model, "legacy");
  assert!(loaded.ui.roster_visible);
}

#[tokio::test]
async fn providers_without_credentials_stay_credential_free() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  let mut config = Config::default();
  config.providers.insert(
    "local".into(),
    ProviderConfig::http("http://localhost/v1", ""),
  );
  config.save_to(&path).await.unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert!(loaded.providers["local"].api_key_env.is_empty());
}

#[tokio::test]
async fn memory_and_synapse_settings_round_trip() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  let mut config = Config::default();
  config.memory.backend = MemoryBackend::Synapse;
  config.memory.teach = true;
  config.memory.recall_limit = 3;
  config.synapse.mesh = true;
  config.save_to(&path).await.unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert_eq!(loaded.memory.backend, MemoryBackend::Synapse);
  assert!(loaded.memory.teach);
  assert_eq!(loaded.memory.recall_limit, 3);
  // the backend implies the integration, so the mesh setting is live without a second switch
  assert!(loaded.synapse_active());
  assert!(loaded.mesh_active());
}

#[tokio::test]
async fn memory_defaults_to_local_and_synapse_stays_off() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(&path, "model = \"tiny\"\n").await.unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert_eq!(loaded.memory.backend, MemoryBackend::Local);
  assert!(loaded.memory.recall_on_start);
  assert!(loaded.memory.remember_on_compact);
  assert!(!loaded.memory.teach);
  assert!(!loaded.synapse_active());
  assert!(!loaded.mesh_active());
}
