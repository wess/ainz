#![cfg(feature = "lua")]

use std::{sync::Arc, time::Duration};

use ainz::{
  PluginCatalog,
  tool::{Tool, ToolContext},
};
use serde_json::json;

async fn plugin(
  source: &str,
  capabilities: &str,
  runtime: &str,
) -> (tempfile::TempDir, PluginCatalog) {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/plugins/script");
  tokio::fs::create_dir_all(root.join("lua")).await.unwrap();
  tokio::fs::write(root.join("main.lua"), source)
    .await
    .unwrap();
  tokio::fs::write(root.join("lua/greeting.lua"), "return { text = 'hello' }")
    .await
    .unwrap();
  let manifest = format!(
    r#"
capabilities = [{capabilities}]
[plugin]
name = "script"
version = "0.1.0"
[runtime]
kind = "lua"
path = "main.lua"
{runtime}
[[tools]]
name = "run"
description = "Run the test"
capabilities = [{capabilities}]
parameters = {{ type = "object" }}
"#
  );
  tokio::fs::write(root.join("plugin.toml"), manifest)
    .await
    .unwrap();
  let grants = temp.path().join("grants.json");
  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  assert!(
    catalog
      .plugins
      .iter()
      .any(|p| p.manifest.plugin.name == "script"),
    "{:?}",
    catalog
  );
  catalog.approve_with("script", &grants).await.unwrap();
  (temp, catalog)
}
async fn tool(catalog: &PluginCatalog) -> Arc<dyn Tool> {
  catalog
    .approved_tools()
    .await
    .unwrap()
    .into_iter()
    .find(|tool| tool.spec().name == "script_run")
    .unwrap()
}
fn context(temp: &tempfile::TempDir) -> ToolContext {
  ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 4096)
}

#[tokio::test]
async fn lua_tools_load_bundled_modules_and_start_fresh() {
  let (temp, catalog) = plugin("local greeting = require('greeting'); local count = 0; return { run = function(args) count = count + 1; return { message = greeting.text .. ' ' .. args.name, count = count } end }", "\"compute\"", "").await;
  let tool = tool(&catalog).await;
  for _ in 0..2 {
    let output = tool
      .execute(&context(&temp), json!({"name":"world"}))
      .await
      .unwrap();
    assert_eq!(
      serde_json::from_str::<serde_json::Value>(&output).unwrap(),
      json!({"message":"hello world", "count":1})
    );
  }
}

#[tokio::test]
async fn lua_sources_and_modules_are_pinned_before_loading() {
  let (temp, catalog) = plugin(
    "return { run = function() return require('greeting').text end }",
    "\"compute\"",
    "",
  )
  .await;
  tokio::fs::write(
    temp.path().join(".ainz/plugins/script/lua/greeting.lua"),
    "return { text = 'changed' }",
  )
  .await
  .unwrap();
  assert!(catalog.approved_tools().await.is_err());
  let changed = PluginCatalog::discover_with_grants(temp.path(), &temp.path().join("grants.json"))
    .await
    .unwrap();
  assert!(
    !changed
      .plugins
      .iter()
      .find(|p| p.manifest.plugin.name == "script")
      .unwrap()
      .approved
  );
}

#[tokio::test]
async fn lua_compute_cannot_access_ambient_authority() {
  let source = "return { run = function() for _, name in ipairs({'io', 'os', 'package', 'debug', 'coroutine', 'dofile', 'loadfile', 'load', 'print'}) do assert(_G[name] == nil, name) end; local ok = pcall(ainz.write, 'marker', 'bad'); assert(not ok); ok = pcall(ainz.run, 'touch marker'); assert(not ok); ok = pcall(ainz.fetch, 'http://example.com'); assert(not ok); return 'isolated' end }";
  let (temp, catalog) = plugin(source, "\"compute\"", "").await;
  assert_eq!(
    tool(&catalog)
      .await
      .execute(&context(&temp), json!({}))
      .await
      .unwrap(),
    "isolated"
  );
  assert!(!temp.path().join("marker").exists());
}

#[tokio::test]
async fn lua_workspace_access_uses_the_host_boundary() {
  let source = "return { run = function(args) ainz.write(args.path, 'hello'); return ainz.read(args.path) end }";
  let (temp, catalog) = plugin(source, "\"workspace_read\", \"workspace_write\"", "").await;
  let tool = tool(&catalog).await;
  assert_eq!(
    tool
      .execute(&context(&temp), json!({"path":"nested/marker"}))
      .await
      .unwrap(),
    "hello"
  );
  assert!(
    tool
      .execute(&context(&temp), json!({"path":"../marker"}))
      .await
      .is_err()
  );
  let outside = tempfile::tempdir().unwrap();
  std::os::unix::fs::symlink(outside.path().join("marker"), temp.path().join("escape")).unwrap();
  assert!(
    tool
      .execute(&context(&temp), json!({"path":"escape"}))
      .await
      .is_err()
  );
  assert!(!outside.path().join("marker").exists());
}

#[tokio::test]
async fn lua_instruction_budget_cannot_be_caught() {
  for source in [
    "while true do end",
    "return { run = function() while true do end end }",
    "return { run = function() while true do pcall(function() while true do end end) end end }",
  ] {
    let (temp, catalog) = plugin(source, "\"compute\"", "fuel = 10000").await;
    let tool = tool(&catalog).await;
    let error = tokio::time::timeout(
      Duration::from_secs(2),
      tool.execute(&context(&temp), json!({})),
    )
    .await
    .expect("Lua failed to yield")
    .unwrap_err();
    assert!(
      error.to_string().contains("instruction budget"),
      "{error:#}"
    );
  }
}

#[tokio::test]
async fn lua_memory_and_time_are_bounded() {
  let (temp, catalog) = plugin(
    "return { run = function() return string.rep('x', 2000000) end }",
    "\"compute\"",
    "memory_bytes = 1048576",
  )
  .await;
  assert!(
    tool(&catalog)
      .await
      .execute(&context(&temp), json!({}))
      .await
      .is_err()
  );
  let (temp, catalog) = plugin(
    "return { run = function() while true do end end }",
    "\"compute\"",
    "timeout_ms = 10\nfuel = 18446744073709551615",
  )
  .await;
  let tool = tool(&catalog).await;
  let error = tokio::time::timeout(
    Duration::from_secs(2),
    tool.execute(&context(&temp), json!({})),
  )
  .await
  .unwrap()
  .unwrap_err();
  assert!(error.to_string().contains("timed out"), "{error:#}");
}

#[tokio::test]
async fn lua_module_names_cannot_escape_the_bundle() {
  let (temp, catalog) = plugin(
    "return { run = function(args) return require(args.name) end }",
    "\"compute\"",
    "",
  )
  .await;
  let tool = tool(&catalog).await;
  for name in ["../outside", "/etc/passwd", "..", "greeting.lua", "ffi"] {
    assert!(
      tool
        .execute(&context(&temp), json!({"name":name}))
        .await
        .is_err()
    );
  }
}

#[tokio::test]
async fn lua_cancellation_terminates_a_running_host_command() {
  let (temp, catalog) = plugin(
    "return { run = function() return ainz.run('(sleep 0.3; touch marker) & wait') end }",
    "\"process_exec\"",
    "",
  )
  .await;
  let tool = tool(&catalog).await;
  assert!(
    tokio::time::timeout(
      Duration::from_millis(100),
      tool.execute(&context(&temp), json!({}))
    )
    .await
    .is_err()
  );
  tokio::time::sleep(Duration::from_millis(500)).await;
  assert!(!temp.path().join("marker").exists());
}

#[tokio::test]
async fn lua_host_calls_enforce_each_tools_capabilities() {
  let source = "return { run = function() return ainz.read('marker') end, other = function() return ainz.read('marker') end }";
  let (temp, _) = plugin(source, "\"workspace_read\"", "").await;
  let manifest = temp.path().join(".ainz/plugins/script/plugin.toml");
  let text = tokio::fs::read_to_string(&manifest)
    .await
    .unwrap()
    .replacen(
      "capabilities = [\"workspace_read\"]",
      "capabilities = [\"workspace_read\", \"compute\"]",
      1,
    );
  tokio::fs::write(&manifest, format!("{text}\n[[tools]]\nname = \"other\"\ndescription = \"No file authority\"\ncapabilities = [\"compute\"]\nparameters = {{ type = \"object\" }}\n")).await.unwrap();
  tokio::fs::write(temp.path().join("marker"), "hello")
    .await
    .unwrap();
  let mut catalog =
    PluginCatalog::discover_with_grants(temp.path(), &temp.path().join("grants.json"))
      .await
      .unwrap();
  catalog
    .approve_with("script", &temp.path().join("grants.json"))
    .await
    .unwrap();
  let tools = catalog.approved_tools().await.unwrap();
  let read = tools
    .iter()
    .find(|t| t.spec().name == "script_run")
    .unwrap();
  assert_eq!(
    read.execute(&context(&temp), json!({})).await.unwrap(),
    "hello"
  );
  let other = tools
    .iter()
    .find(|t| t.spec().name == "script_other")
    .unwrap();
  assert!(other.execute(&context(&temp), json!({})).await.is_err());
}

#[tokio::test]
async fn lua_progress_is_bounded_across_many_log_calls() {
  let (temp, catalog) = plugin("return { run = function() for i = 1, 10000 do ainz.log(string.rep('x', 100)) end return 'done' end }", "\"compute\"", "").await;
  let (events, mut receiver) = ainz::EventSink::channel();
  let mut context = context(&temp);
  context.progress = Some((events, "log".into()));
  assert_eq!(
    tool(&catalog)
      .await
      .execute(&context, json!({}))
      .await
      .unwrap(),
    "done"
  );
  let mut length = 0;
  while let Ok(ainz::Event::ToolDelta { text, .. }) = receiver.try_recv() {
    length += text.len();
  }
  assert!(length <= context.max_output_bytes + 32);
}

#[tokio::test]
async fn lua_rejects_bytecode_and_circular_modules() {
  let (temp, catalog) = plugin("\u{1b}Lua invalid bytecode", "\"compute\"", "").await;
  assert!(
    tool(&catalog)
      .await
      .execute(&context(&temp), json!({}))
      .await
      .is_err()
  );
  let (temp, _) = plugin(
    "return { run = function() return require('greeting') end }",
    "\"compute\"",
    "",
  )
  .await;
  tokio::fs::write(
    temp.path().join(".ainz/plugins/script/lua/greeting.lua"),
    "return require('greeting')",
  )
  .await
  .unwrap();
  let grants = temp.path().join("grants.json");
  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  catalog.approve_with("script", &grants).await.unwrap();
  let error = tool(&catalog)
    .await
    .execute(&context(&temp), json!({}))
    .await
    .unwrap_err();
  assert!(error.to_string().contains("circular"), "{error:#}");
}
