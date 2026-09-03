use ainz::{Config, Credential, ProviderConfig};

fn round_trip(credential: Credential) -> ProviderConfig {
  let mut original = ProviderConfig::http("http://localhost/v1", "");
  original.credential = Some(credential);
  let text = toml::to_string(&original).unwrap();
  let restored: ProviderConfig = toml::from_str(&text).unwrap();
  assert_eq!(restored.credential, original.credential);
  restored
}

#[test]
fn none_round_trips() {
  round_trip(Credential::None);
}

#[test]
fn env_round_trips() {
  round_trip(Credential::Env {
    var: "OPENROUTER_API_KEY".into(),
  });
}

#[test]
fn synapse_round_trips() {
  round_trip(Credential::Synapse {
    secret: "apis.OpenRouter".into(),
    var: "OPENROUTER_API_KEY".into(),
  });
}

#[test]
fn keychain_round_trips() {
  round_trip(Credential::Keychain {
    account: "openrouter".into(),
  });
}

// what actually lands on disk, since that shape is the compatibility contract with hand-edited
// config files
#[test]
fn saved_toml_names_the_source_and_nothing_else() {
  let mut provider = ProviderConfig::http("http://localhost/v1", "");
  provider.credential = Some(Credential::Env {
    var: "OPENROUTER_API_KEY".into(),
  });
  let text = toml::to_string(&provider).unwrap();
  assert!(text.contains("[credential]"));
  assert!(text.contains("from = \"env\""));
  assert!(text.contains("var = \"OPENROUTER_API_KEY\""));

  let mut provider = ProviderConfig::http("http://localhost/v1", "");
  provider.credential = Some(Credential::Synapse {
    secret: "apis.OpenRouter".into(),
    var: "OPENROUTER_API_KEY".into(),
  });
  let text = toml::to_string(&provider).unwrap();
  assert!(text.contains("from = \"synapse\""));
  assert!(text.contains("secret = \"apis.OpenRouter\""));

  let mut provider = ProviderConfig::http("http://localhost/v1", "");
  provider.credential = Some(Credential::Keychain {
    account: "openrouter".into(),
  });
  let text = toml::to_string(&provider).unwrap();
  assert!(text.contains("from = \"keychain\""));
  assert!(text.contains("account = \"openrouter\""));
}

// a provider with no credential set at all omits the table entirely, so an old config with just
// `api_key_env` is untouched by round-tripping through this type
#[test]
fn absent_credential_is_not_written() {
  let provider = ProviderConfig::http("http://localhost/v1", "OPENROUTER_API_KEY");
  let text = toml::to_string(&provider).unwrap();
  assert!(!text.contains("credential"));
  assert!(text.contains("api_key_env = \"OPENROUTER_API_KEY\""));
}

#[tokio::test]
async fn env_credential_resolves_a_set_variable() {
  unsafe { std::env::set_var("AINZ_CREDENTIAL_TEST_SET", "sk-test-value") };
  let credential = Credential::Env {
    var: "AINZ_CREDENTIAL_TEST_SET".into(),
  };
  assert_eq!(
    credential.resolve().await.unwrap(),
    Some("sk-test-value".into())
  );
  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_SET") };
}

#[tokio::test]
async fn env_credential_resolves_an_unset_variable_to_none() {
  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_UNSET") };
  let credential = Credential::Env {
    var: "AINZ_CREDENTIAL_TEST_UNSET".into(),
  };
  assert_eq!(credential.resolve().await.unwrap(), None);
}

#[tokio::test]
async fn env_credential_resolves_an_empty_variable_to_none() {
  unsafe { std::env::set_var("AINZ_CREDENTIAL_TEST_EMPTY", "") };
  let credential = Credential::Env {
    var: "AINZ_CREDENTIAL_TEST_EMPTY".into(),
  };
  assert_eq!(credential.resolve().await.unwrap(), None);
  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_EMPTY") };
}

#[tokio::test]
async fn no_credential_resolves_to_none() {
  assert_eq!(Credential::None.resolve().await.unwrap(), None);
}

// old config files only ever had `api_key_env`; a provider loaded from one of them has
// `credential: None`, and `api_key_for` must still find the key through that name
#[tokio::test]
async fn a_provider_with_no_credential_still_resolves_through_api_key_env() {
  unsafe { std::env::set_var("AINZ_CREDENTIAL_TEST_FALLBACK", "legacy-value") };
  let config = Config::default();
  let provider = ProviderConfig::http("http://localhost/v1", "AINZ_CREDENTIAL_TEST_FALLBACK");
  assert!(provider.credential.is_none());

  let key = config.api_key_for(&provider).await.unwrap();

  assert_eq!(key, Some("legacy-value".into()));
  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_FALLBACK") };
}

#[tokio::test]
async fn a_provider_with_no_credential_and_no_api_key_env_resolves_to_none() {
  let config = Config::default();
  let provider = ProviderConfig::http("http://localhost/v1", "");

  let key = config.api_key_for(&provider).await.unwrap();

  assert_eq!(key, None);
}

// an explicit credential wins over api_key_env, even when both are present
#[tokio::test]
async fn an_explicit_credential_takes_priority_over_api_key_env() {
  unsafe { std::env::set_var("AINZ_CREDENTIAL_TEST_WINNER", "from-credential") };
  unsafe { std::env::set_var("AINZ_CREDENTIAL_TEST_LOSER", "from-api-key-env") };
  let config = Config::default();
  let mut provider = ProviderConfig::http("http://localhost/v1", "AINZ_CREDENTIAL_TEST_LOSER");
  provider.credential = Some(Credential::Env {
    var: "AINZ_CREDENTIAL_TEST_WINNER".into(),
  });

  let key = config.api_key_for(&provider).await.unwrap();

  assert_eq!(key, Some("from-credential".into()));
  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_WINNER") };
  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_LOSER") };
}

// Debug must never print anything that could be a secret value, even when the field it names
// happens, at resolve time, to point at one
#[test]
fn debug_never_includes_a_value() {
  let secret = "sk-do-not-print-me";
  unsafe { std::env::set_var("AINZ_CREDENTIAL_TEST_DEBUG", secret) };

  let variants = [
    Credential::None,
    Credential::Env {
      var: "AINZ_CREDENTIAL_TEST_DEBUG".into(),
    },
    Credential::Synapse {
      secret: "apis.OpenRouter".into(),
      var: "AINZ_CREDENTIAL_TEST_DEBUG".into(),
    },
    Credential::Keychain {
      account: "openrouter".into(),
    },
  ];

  for variant in &variants {
    let rendered = format!("{variant:?}");
    assert!(
      !rendered.contains(secret),
      "debug leaked a value: {rendered}"
    );
  }

  unsafe { std::env::remove_var("AINZ_CREDENTIAL_TEST_DEBUG") };
}
