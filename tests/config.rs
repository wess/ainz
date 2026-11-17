use ainz::{Config, MemoryBackend, ProcessOutput, ProviderConfig};

#[tokio::test]
async fn automatic_improvement_is_opt_in_and_its_limits_round_trip() {
  let temp = tempfile::tempdir().unwrap();
  let path = temp.path().join("config.toml");
  let mut config = Config::default();
  assert!(!config.improve.enabled);
  config.improve.enabled = true;
  config.improve.source = Some(temp.path().join("source"));
  config.improve.every_runs = 3;
  config.improve.checks = vec!["cargo test --no-default-features".into()];
  config.save_to(&path).await.unwrap();
  assert_eq!(
    Config::load_from(&path).await.unwrap().improve,
    config.improve
  );
  config.improve.budget_usd = f64::NAN;
  assert!(config.improve.validate().is_err());
}

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

#[tokio::test]
async fn the_buffered_claude_preset_is_moved_to_the_streaming_one() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(
    &path,
    r#"
provider = "claude"
model = "opus"

[providers.claude]
kind = "process"
command = "claude"
args = ["-p", "--output-format", "json", "--no-session-persistence", "--model", "{model}", "--permission-mode", "{permission}"]
output = "json_result"
models = ["sonnet", "opus"]

[providers.mine]
kind = "process"
command = "claude"
args = ["-p", "--output-format", "json"]
output = "json_result"
"#,
  )
  .await
  .unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  let claude = &loaded.providers["claude"];
  assert_eq!(claude.output, ProcessOutput::StreamJson);
  assert!(claude.args.iter().any(|arg| arg == "stream-json"));
  assert!(
    claude
      .args
      .iter()
      .any(|arg| arg == "--include-partial-messages")
  );
  assert_eq!(claude.models, ["sonnet", "opus"]);
  // a hand-written profile is left exactly as its owner wrote it
  let mine = &loaded.providers["mine"];
  assert_eq!(mine.output, ProcessOutput::JsonResult);
  assert_eq!(mine.args, ["-p", "--output-format", "json"]);
}

#[tokio::test]
async fn a_saved_claude_preset_learns_to_read_images() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(
    &path,
    r#"
provider = "claude"
model = "sonnet"

[providers.claude]
kind = "process"
command = "claude"
args = ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence", "--model", "{model}", "--permission-mode", "{permission}"]
output = "stream_json"

[providers.mine]
kind = "process"
command = "claude"
args = ["-p", "--output-format", "stream-json", "--verbose"]
output = "stream_json"
"#,
  )
  .await
  .unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert_eq!(
    loaded.providers["claude"].args,
    ainz::config::ProviderConfig::claude_code().args
  );
  assert_eq!(
    loaded.providers["mine"].args,
    ["-p", "--output-format", "stream-json", "--verbose"]
  );
}

#[tokio::test]
async fn a_claude_preset_from_before_the_permission_bridge_gains_it_and_others_are_left_alone() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(
    &path,
    r#"
provider = "claude"
model = "sonnet"

[providers.claude]
kind = "process"
command = "claude"
args = ["-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence", "--model", "{model}", "--permission-mode", "{permission}"]
output = "stream_json"

[providers.mine]
kind = "process"
command = "claude"
args = ["-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose", "--model", "{model}"]
output = "stream_json"
"#,
  )
  .await
  .unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  let preset = ainz::config::ProviderConfig::claude_code().args;
  assert_eq!(loaded.providers["claude"].args, preset);
  assert!(
    preset
      .windows(2)
      .any(|pair| pair == ["--permission-prompt-tool", "stdio"])
  );
  assert!(!loaded.providers["mine"].args.contains(&"stdio".to_string()));
}

#[tokio::test]
async fn a_profile_s_output_limit_round_trips_and_only_applies_to_chat() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(
    &path,
    r#"
provider = "proxy"
model = "m"

[providers.proxy]
kind = "http"
endpoint = "http://127.0.0.1:4000/v1"
max_output_tokens = 64000

[providers.messages]
kind = "http"
wire_api = "messages"
endpoint = "https://api.anthropic.com/v1"
max_output_tokens = 64000
"#,
  )
  .await
  .unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert_eq!(loaded.providers["proxy"].max_output_tokens, Some(64_000));
  assert!(loaded.providers["proxy"].validate("proxy").is_ok());
  assert!(loaded.providers["messages"].validate("messages").is_err());
}

#[test]
fn a_standing_rule_decides_before_anyone_is_asked() {
  let rules = ainz::PermissionRules {
    allow: vec![
      "read".into(),
      "shell(git *)".into(),
      "write(notes.md)".into(),
    ],
    deny: vec!["shell(rm *)".into()],
  };

  assert_eq!(rules.decide("read", None), Some(true));
  assert_eq!(rules.decide("shell", Some("git status")), Some(true));
  assert_eq!(rules.decide("write", Some("notes.md")), Some(true));
  // a prefix rule is a prefix, not a substring
  assert_eq!(rules.decide("shell", Some("cd x && git status")), None);
  assert_eq!(rules.decide("write", Some("other.md")), None);
  assert_eq!(rules.decide("edit", Some("notes.md")), None);
  // deny wins over an allowance for the same tool
  assert_eq!(rules.decide("shell", Some("rm -rf /")), Some(false));
}

#[test]
fn the_rule_offered_for_a_call_is_one_a_person_can_mean() {
  assert_eq!(
    ainz::PermissionRules::rule_for("shell", Some("git status --short")),
    "shell(git *)"
  );
  assert_eq!(
    ainz::PermissionRules::rule_for("read", Some("a.txt")),
    "read"
  );
  assert_eq!(ainz::PermissionRules::rule_for("shell", None), "shell");
}

#[tokio::test]
async fn permission_rules_round_trip() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  let mut config = Config::default();
  config.rules.allow.push("shell(cargo *)".into());
  config.save_to(&path).await.unwrap();

  let loaded = Config::load_from(&path).await.unwrap();

  assert_eq!(loaded.rules.allow, ["shell(cargo *)"]);
  assert!(loaded.rules.deny.is_empty());
}

#[tokio::test]
async fn a_session_s_scripts_and_flags_stay_out_of_config_toml() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(&path, "model = \"from-file\"\nmax_steps = 10\n")
    .await
    .unwrap();
  let mut config = Config::load_from(&path).await.unwrap();
  // what init.lua or --model set for this session
  config.model = "from-init-lua".into();
  config.settle();
  // what the person then changed in /settings
  config.ui.vim = true;
  config.save_to(&path).await.unwrap();

  let saved = Config::load_from(&path).await.unwrap();
  assert_eq!(saved.model, "from-file");
  assert!(saved.ui.vim);
  assert_eq!(saved.max_steps, 10);
}

#[tokio::test]
async fn an_option_a_script_changes_mid_session_is_not_saved_either() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(&path, "model = \"m\"\n").await.unwrap();
  let mut config = Config::load_from(&path).await.unwrap();
  config.settle();
  let mut scripted = config.clone();
  scripted.ui.bell = false;
  config.adopt(scripted);
  config.max_steps = 77;
  config.save_to(&path).await.unwrap();

  let saved = Config::load_from(&path).await.unwrap();
  assert!(saved.ui.bell, "the script's change belonged to the session");
  assert_eq!(saved.max_steps, 77);
}

#[tokio::test]
async fn a_codex_preset_from_before_the_git_check_was_skipped_gains_it() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  tokio::fs::write(
    &path,
    r#"
provider = "codex"
model = "gpt"

[providers.codex]
kind = "process"
command = "codex"
args = ["exec", "--ephemeral", "--color", "never", "--sandbox", "{sandbox}", "-C", "{workspace}", "--model", "{model}", "-"]
"#,
  )
  .await
  .unwrap();
  let loaded = Config::load_from(&path).await.unwrap();
  assert_eq!(
    loaded.providers["codex"].args,
    ainz::config::ProviderConfig::codex().args
  );
}
