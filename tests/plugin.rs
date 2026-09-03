use std::os::unix::fs::PermissionsExt;

use ainz::{McpProfile, PluginCatalog, PluginFormat, SkillCatalog, tool::ToolContext};
use serde_json::json;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
};

const MANIFEST: &str = r#"
capabilities = ["workspace_read"]

[plugin]
name = "echo"
version = "0.1.0"

[runtime]
command = ["run.sh"]

[[tools]]
name = "say"
description = "Return a value"
capabilities = ["workspace_read"]
parameters = { type = "object" }
"#;

#[tokio::test]
async fn plugins_require_content_pinned_approval() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/plugins/echo");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(root.join("plugin.toml"), MANIFEST)
    .await
    .unwrap();
  let runner = root.join("run.sh");
  tokio::fs::write(
    &runner,
    "#!/bin/sh\nread request\nhead -c 131072 /dev/zero >&2\nprintf '{\"result\":{\"ok\":true}}\\n'\n",
  )
  .await
  .unwrap();
  std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
  let grants = temp.path().join("grants.json");

  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let plugin = catalog
    .plugins
    .iter()
    .find(|plugin| plugin.manifest.plugin.name == "echo")
    .unwrap();
  assert!(!plugin.approved);
  catalog.approve_with("echo", &grants).await.unwrap();

  let catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let tool = catalog
    .approved_tools()
    .await
    .unwrap()
    .into_iter()
    .find(|tool| tool.spec().name == "echo_say")
    .unwrap();
  let output = tool
    .execute(
      &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
      json!({"message": "hello"}),
    )
    .await
    .unwrap();
  assert_eq!(output, r#"{"ok":true}"#);

  tokio::fs::write(&runner, "#!/bin/sh\nexit 1\n")
    .await
    .unwrap();
  let changed = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let plugin = changed
    .plugins
    .iter()
    .find(|plugin| plugin.manifest.plugin.name == "echo")
    .unwrap();
  assert!(!plugin.approved);
}

#[tokio::test]
async fn agent_plugins_load_portable_skills_and_mcp_configuration() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".agents/plugins/portable-tools");
  let skill = root.join("skills/greet");
  tokio::fs::create_dir_all(&skill).await.unwrap();
  tokio::fs::create_dir_all(root.join("bin")).await.unwrap();
  tokio::fs::write(
    root.join("plugin.json"),
    r#"{
      "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
      "name": "portable-tools",
      "version": "1.2.3"
    }"#,
  )
  .await
  .unwrap();
  tokio::fs::write(
    skill.join("SKILL.md"),
    "---\nname: greet\ndescription: Greet someone\n---\n\n# Greet\n",
  )
  .await
  .unwrap();
  tokio::fs::write(
    root.join("mcp.json"),
    r#"{
      "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
      "mcpServers": {
        "echo": {
          "type": "stdio",
          "command": "./bin/server",
          "args": ["--root", "${PLUGIN_ROOT}", "--data", "${PLUGIN_DATA}"],
          "env": {"PLUGIN_CONFIG": "${PLUGIN_ROOT}/config.json"},
          "cwd": "${PLUGIN_ROOT}"
        }
      }
    }"#,
  )
  .await
  .unwrap();
  let grants = temp.path().join("grants.json");

  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let plugin = catalog
    .plugins
    .iter()
    .find(|plugin| plugin.manifest.plugin.name == "portable-tools")
    .unwrap();
  assert_eq!(plugin.format, PluginFormat::AgentPlugin);
  assert!(!plugin.approved);
  catalog
    .approve_with("portable-tools", &grants)
    .await
    .unwrap();

  let catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let skills = SkillCatalog::discover_with_roots(temp.path(), &catalog.approved_skill_roots())
    .await
    .unwrap();
  assert!(skills.skills.iter().any(|skill| skill.name == "greet"));

  let profile = catalog.merge_mcp(McpProfile::default()).await.unwrap();
  let server = &profile.servers["portable-tools__echo"];
  assert_eq!(server.command[0], root.join("bin/server").to_string_lossy());
  assert_eq!(server.cwd.as_deref(), Some(root.as_path()));
  assert_eq!(
    server.env["PLUGIN_CONFIG"],
    root.join("config.json").to_string_lossy()
  );
  assert_eq!(server.env["PLUGIN_ROOT"], root.to_string_lossy());

  tokio::fs::write(skill.join("SKILL.md"), "changed")
    .await
    .unwrap();
  let changed = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  assert!(
    !changed
      .plugins
      .iter()
      .find(|plugin| plugin.manifest.plugin.name == "portable-tools")
      .unwrap()
      .approved
  );
}

#[tokio::test]
async fn component_plugins_run_in_the_sandbox() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/plugins/component_echo");
  tokio::fs::create_dir_all(&root).await.unwrap();
  let artifact = root.join("echo.wasm");
  tokio::fs::write(&artifact, include_bytes!("fixtures/echo.wasm"))
    .await
    .unwrap();
  tokio::fs::write(
    root.join("plugin.toml"),
    r#"
capabilities = ["compute", "workspace_read", "workspace_write", "process_exec", "network"]

[plugin]
name = "component_echo"
version = "0.1.0"

[runtime]
kind = "component"
path = "echo.wasm"
timeout_ms = 1000
memory_bytes = 8388608
fuel = 10000000

[[tools]]
name = "echo"
description = "Echo a value"
capabilities = ["compute"]
parameters = { type = "object" }

[[tools]]
name = "read"
description = "Read a workspace file"
capabilities = ["workspace_read"]
parameters = { type = "object", properties = { path = { type = "string" } } }

[[tools]]
name = "denied_read"
description = "Attempt a read without authority"
capabilities = ["compute"]
parameters = { type = "object", properties = { path = { type = "string" } } }

[[tools]]
name = "write"
description = "Write a workspace file"
capabilities = ["workspace_write"]
parameters = { type = "object", properties = { path = { type = "string" }, content = { type = "string" } } }

[[tools]]
name = "run"
description = "Run a workspace command"
capabilities = ["process_exec"]
parameters = { type = "object", properties = { command = { type = "string" } } }

[[tools]]
name = "fetch"
description = "Fetch an HTTP resource"
capabilities = ["network"]
parameters = { type = "object", properties = { url = { type = "string" } } }
"#,
  )
  .await
  .unwrap();
  let grants = temp.path().join("grants.json");
  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  catalog
    .approve_with("component_echo", &grants)
    .await
    .unwrap();

  let catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let tool = catalog
    .approved_tools()
    .await
    .unwrap()
    .into_iter()
    .find(|tool| tool.spec().name == "component_echo_echo")
    .unwrap();
  let output = tool
    .execute(
      &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
      json!({"message": "hello"}),
    )
    .await
    .unwrap();
  assert_eq!(output, r#"{"message":"hello"}"#);

  tokio::fs::write(temp.path().join("input.txt"), "host read")
    .await
    .unwrap();
  let tools = catalog.approved_tools().await.unwrap();
  let read = tools
    .iter()
    .find(|tool| tool.spec().name == "component_echo_read")
    .unwrap();
  assert_eq!(read.risk(&json!({})), ainz::tool::Risk::Read);
  assert_eq!(
    read
      .execute(
        &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
        json!({"path": "input.txt"}),
      )
      .await
      .unwrap(),
    "host read"
  );
  let denied = tools
    .iter()
    .find(|tool| tool.spec().name == "component_echo_denied_read")
    .unwrap();
  let error = denied
    .execute(
      &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
      json!({"path": "input.txt"}),
    )
    .await
    .unwrap_err();
  assert!(
    error
      .to_string()
      .contains("WorkspaceRead capability is required")
  );
  let write = tools
    .iter()
    .find(|tool| tool.spec().name == "component_echo_write")
    .unwrap();
  write
    .execute(
      &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
      json!({"path": "nested/output.txt", "content": "host write"}),
    )
    .await
    .unwrap();
  assert_eq!(
    tokio::fs::read_to_string(temp.path().join("nested/output.txt"))
      .await
      .unwrap(),
    "host write"
  );
  let run = tools
    .iter()
    .find(|tool| tool.spec().name == "component_echo_run")
    .unwrap();
  assert_eq!(run.risk(&json!({})), ainz::tool::Risk::Execute);
  assert!(
    run
      .execute(
        &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
        json!({"command": "printf component-process"}),
      )
      .await
      .unwrap()
      .contains("component-process")
  );
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = [0_u8; 4096];
    let _ = socket.read(&mut request).await.unwrap();
    let body = "component-network";
    socket
      .write_all(
        format!(
          "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
          body.len()
        )
        .as_bytes(),
      )
      .await
      .unwrap();
  });
  let fetch = tools
    .iter()
    .find(|tool| tool.spec().name == "component_echo_fetch")
    .unwrap();
  assert_eq!(fetch.risk(&json!({})), ainz::tool::Risk::Network);
  assert_eq!(
    fetch
      .execute(
        &ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024),
        json!({"url": format!("http://{address}/value")}),
      )
      .await
      .unwrap(),
    "component-network"
  );
  server.await.unwrap();

  let mut changed = tokio::fs::read(&artifact).await.unwrap();
  changed.push(0);
  tokio::fs::write(&artifact, changed).await.unwrap();
  let catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let plugin = catalog
    .plugins
    .iter()
    .find(|plugin| plugin.manifest.plugin.name == "component_echo")
    .unwrap();
  assert!(!plugin.approved);
}

#[tokio::test]
async fn a_broken_manifest_is_reported_without_hiding_the_others() {
  let temp = tempfile::tempdir().unwrap();
  let good = temp.path().join(".ainz/plugins/echo");
  tokio::fs::create_dir_all(&good).await.unwrap();
  tokio::fs::write(good.join("plugin.toml"), MANIFEST)
    .await
    .unwrap();
  tokio::fs::write(good.join("run.sh"), "#!/bin/sh\n")
    .await
    .unwrap();
  let bad = temp.path().join(".ainz/plugins/broken");
  tokio::fs::create_dir_all(&bad).await.unwrap();
  tokio::fs::write(bad.join("plugin.toml"), "this is not toml = [")
    .await
    .unwrap();
  let grants = temp.path().join("grants.json");

  let catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();

  assert_eq!(catalog.plugins.len(), 1);
  assert_eq!(catalog.plugins[0].manifest.plugin.name, "echo");
  assert_eq!(catalog.issues.len(), 1);
  assert!(catalog.issues[0].contains("broken"));
}

#[tokio::test]
async fn a_program_swapped_after_approval_is_refused_at_run_time() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/plugins/echo");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(root.join("plugin.toml"), MANIFEST)
    .await
    .unwrap();
  let runner = root.join("run.sh");
  tokio::fs::write(
    &runner,
    "#!/bin/sh\nread request\nprintf '{\"result\":\"first\"}\\n'\n",
  )
  .await
  .unwrap();
  std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
  let grants = temp.path().join("grants.json");
  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  catalog.approve_with("echo", &grants).await.unwrap();
  let catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  let tool = catalog
    .approved_tools()
    .await
    .unwrap()
    .into_iter()
    .find(|tool| tool.spec().name == "echo_say")
    .unwrap();
  let context = ToolContext::new(temp.path().into(), uuid::Uuid::nil(), 1024);
  assert_eq!(tool.execute(&context, json!({})).await.unwrap(), "first");

  // the model, or anyone else, rewrites the program mid-session
  tokio::fs::write(
    &runner,
    "#!/bin/sh\nread request\nprintf '{\"result\":\"swapped\"}\\n'\n",
  )
  .await
  .unwrap();
  let error = tool.execute(&context, json!({})).await.unwrap_err();
  assert!(
    error
      .to_string()
      .contains("changed since the plugin was approved")
  );
}

#[tokio::test]
async fn yeet_loads_unapproved_plugins_without_granting_them() {
  let temp = tempfile::tempdir().unwrap();
  let root = temp.path().join(".ainz/plugins/echo");
  tokio::fs::create_dir_all(&root).await.unwrap();
  tokio::fs::write(root.join("plugin.toml"), MANIFEST)
    .await
    .unwrap();
  let runner = root.join("run.sh");
  tokio::fs::write(
    &runner,
    "#!/bin/sh\nread request\nprintf '{\"result\":{\"ok\":true}}\\n'\n",
  )
  .await
  .unwrap();
  std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
  let grants = temp.path().join("grants.json");

  let mut catalog = PluginCatalog::discover_with_grants(temp.path(), &grants)
    .await
    .unwrap();
  assert!(catalog.approved_tools().await.unwrap().is_empty());

  catalog.trust_all();
  assert!(
    catalog
      .approved_tools()
      .await
      .unwrap()
      .iter()
      .any(|tool| tool.spec().name == "echo_say")
  );
  // the pin is untouched, so the next run without the flag is back to pending
  assert!(!grants.exists());
  assert!(
    !PluginCatalog::discover_with_grants(temp.path(), &grants)
      .await
      .unwrap()
      .plugins[0]
      .approved
  );
}
