use std::sync::Arc;

use ainz::{
  Agent, ChatProvider, Config, EventSink, RunOptions, Session, deny_all,
  protocol::{Message, ToolSpec},
  provider::ProviderReply,
  tool::{ToolSet, builtins},
};
use async_trait::async_trait;

struct Unreachable;

#[async_trait]
impl ChatProvider for Unreachable {
  async fn complete(
    &self,
    _messages: &[Message],
    _tools: &[ToolSpec],
    _events: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    panic!("an invalid run must not reach the provider")
  }
}

#[tokio::test]
async fn invalid_host_options_leave_the_session_untouched() {
  let workspace = tempfile::tempdir().unwrap();
  let agent = Agent::new(
    Unreachable,
    ToolSet::default(),
    workspace.path().into(),
    EventSink::default(),
    deny_all(),
  );
  let mut session = Session::new(workspace.path().into());
  let before = serde_json::to_value(&session).unwrap();
  for options in [
    RunOptions {
      max_steps: 0,
      ..Default::default()
    },
    RunOptions {
      compact_at_tokens: 0,
      ..Default::default()
    },
    RunOptions {
      context_tokens: 1,
      ..Default::default()
    },
    RunOptions {
      preserve_messages: 1,
      ..Default::default()
    },
  ] {
    assert!(
      agent
        .run(&mut session, "hello".into(), options)
        .await
        .is_err()
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
  }
}

#[test]
fn host_configuration_preserves_policy_and_limits() {
  let mut config = Config {
    max_steps: 7,
    ..Default::default()
  };
  config.rules.deny.push("shell".into());
  let options = RunOptions::from(&config);
  options.validate().unwrap();
  assert_eq!(options.max_steps, 7);
  assert_eq!(options.rules.decide("shell", None), Some(false));
  assert!(options.instructions.is_empty());
  assert!(options.memory_nudge.is_none());
}

#[test]
fn rejecting_a_duplicate_tool_preserves_the_registered_instance() {
  let mut tools = ToolSet::default();
  let original = builtins().remove(0);
  let name = original.spec().name;
  tools.insert(original.clone()).unwrap();
  assert!(tools.insert(builtins().remove(0)).is_err());
  assert!(Arc::ptr_eq(tools.get(&name).unwrap(), &original));
}

#[cfg(any(not(feature = "lua"), not(feature = "wasm")))]
#[tokio::test]
async fn disabled_runtimes_can_be_discovered_but_cannot_execute() {
  for (kind, filename, bytes, feature) in [
    ("lua", "main.lua", b"return {}".as_slice(), "lua"),
    ("component", "main.wasm", b"unused".as_slice(), "wasm"),
  ] {
    if (feature == "lua" && cfg!(feature = "lua")) || (feature == "wasm" && cfg!(feature = "wasm"))
    {
      continue;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(".ainz/plugins/demo");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join(filename), bytes).await.unwrap();
    tokio::fs::write(
      root.join("plugin.toml"),
      format!(
        r#"
capabilities = ["compute"]
[plugin]
name = "demo"
version = "0.1.0"
[runtime]
kind = "{kind}"
path = "{filename}"
[[tools]]
name = "test"
description = "test"
capabilities = ["compute"]
parameters = {{ type = "object" }}
"#
      ),
    )
    .await
    .unwrap();
    let mut catalog = ainz::PluginCatalog::discover(temp.path()).await.unwrap();
    assert_eq!(catalog.plugins.len(), 1);
    assert!(catalog.approved_tools().await.unwrap().is_empty());
    catalog.trust_all();
    let error = catalog
      .approved_tools()
      .await
      .err()
      .expect("runtime is unavailable");
    assert!(
      error
        .to_string()
        .contains(&format!("requires the {feature} feature"))
    );
  }
}
