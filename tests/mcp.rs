use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, sync::Arc};

use ainz::{
  McpHub, McpProfile, McpServerConfig, McpTransport,
  tool::{Risk, ToolContext},
};
use serde_json::json;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn mcp_tools_are_discovered_lazily_and_called() {
  let temp = tempfile::tempdir().unwrap();
  let server = temp.path().join("server.sh");
  tokio::fs::write(
    &server,
    r#"#!/bin/sh
while IFS= read -r request; do
  case "$request" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"},"instructions":"Recall project memory before acting."}}'
      ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}},"annotations":{"readOnlyHint":true}},{"name":"wipe","description":"Wipe","inputSchema":{"type":"object"}}]}}'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"called"}],"isError":false}}'
      ;;
  esac
done
"#,
  )
  .await
  .unwrap();
  std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
  let profile = McpProfile {
    servers: BTreeMap::from([
      (
        "broken".into(),
        McpServerConfig {
          transport: McpTransport::Stdio,
          command: vec![temp.path().join("missing").to_string_lossy().into_owned()],
          url: None,
          header_env: BTreeMap::new(),
          headers: BTreeMap::new(),
          env: BTreeMap::new(),
          cwd: None,
          enabled: true,
          required: false,
          timeout_ms: 1_000,
        },
      ),
      (
        "fixture".into(),
        McpServerConfig {
          transport: McpTransport::Stdio,
          command: vec![server.to_string_lossy().into_owned()],
          url: None,
          header_env: BTreeMap::new(),
          headers: BTreeMap::new(),
          env: BTreeMap::new(),
          cwd: None,
          enabled: true,
          required: true,
          timeout_ms: 1_000,
        },
      ),
    ]),
  };
  let hub = Arc::new(McpHub::new(profile));
  assert_eq!(
    hub.instructions().await.unwrap(),
    [(
      "fixture".to_string(),
      "Recall project memory before acting.".to_string()
    )]
  );
  let tool = hub.tool();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };

  let search = tool
    .execute(&context, json!({"command": "search", "query": "echo"}))
    .await
    .unwrap();
  assert_eq!(search, "fixture/echo: Echo text");
  let schema = tool
    .execute(
      &context,
      json!({
        "command": "schema", "server": "fixture", "name": "echo"
      }),
    )
    .await
    .unwrap();
  assert!(schema.contains("input_schema"));
  assert_eq!(tool.risk(&json!({"command": "call"})), Risk::Execute);
  assert_eq!(
    tool.risk(&json!({"command": "call", "server": "fixture", "name": "echo"})),
    Risk::Read
  );
  assert_eq!(
    tool.risk(&json!({"command": "call", "server": "fixture", "name": "wipe"})),
    Risk::Execute
  );
  assert_eq!(
    tool.risk(&json!({"command": "call", "server": "nowhere", "name": "echo"})),
    Risk::Execute
  );
  let result = tool
    .execute(
      &context,
      json!({
        "command": "call", "server": "fixture", "name": "echo",
        "arguments": {"text": "hello"}
      }),
    )
    .await
    .unwrap();
  assert_eq!(result, "called");
}

#[tokio::test]
async fn external_launch_configuration_is_merged_and_required() {
  let temp = tempfile::tempdir().unwrap();
  let path = temp.path().join("launch.mcp.json");
  tokio::fs::write(
    &path,
    serde_json::to_vec(&json!({
      "mcpServers": {
        "synapse": {
          "command": "/usr/local/bin/synapse",
          "args": ["mcp"],
          "env": {"SYNAPSE_PROJECT_DIR": "/tmp/project"}
        }
      }
    }))
    .unwrap(),
  )
  .await
  .unwrap();

  let profile = McpProfile::load_with(Some(&path)).await.unwrap();
  let server = &profile.servers["synapse"];

  assert_eq!(server.command, ["/usr/local/bin/synapse", "mcp"]);
  assert_eq!(server.env["SYNAPSE_PROJECT_DIR"], "/tmp/project");
  assert!(server.required);
}

#[tokio::test]
async fn profile_serializes_in_conventional_mcp_shape() {
  let temp = tempfile::tempdir().unwrap();
  let path = temp.path().join("mcp.toml");
  let profile = McpProfile {
    servers: BTreeMap::from([(
      "synapse".into(),
      McpServerConfig {
        transport: McpTransport::Stdio,
        command: vec!["/opt/synapse".into(), "mcp".into()],
        url: None,
        header_env: BTreeMap::new(),
        headers: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: None,
        enabled: true,
        required: true,
        timeout_ms: 30_000,
      },
    )]),
  };

  profile.save_to(&path).await.unwrap();
  let text = tokio::fs::read_to_string(&path).await.unwrap();
  let loaded: McpProfile = toml::from_str(&text).unwrap();

  assert!(text.contains("command = \"/opt/synapse\""));
  assert!(text.contains("args = [\"mcp\"]"));
  assert_eq!(loaded.servers["synapse"].command, ["/opt/synapse", "mcp"]);
}

#[tokio::test]
async fn required_mcp_servers_are_checked_eagerly() {
  let temp = tempfile::tempdir().unwrap();
  let profile = McpProfile {
    servers: BTreeMap::from([(
      "required".into(),
      McpServerConfig {
        transport: McpTransport::Stdio,
        command: vec![temp.path().join("missing").to_string_lossy().into_owned()],
        url: None,
        header_env: BTreeMap::new(),
        headers: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: None,
        enabled: true,
        required: true,
        timeout_ms: 1_000,
      },
    )]),
  };

  let error = McpHub::new(profile).ready().await.unwrap_err();
  assert!(error.to_string().contains("start server required"));
}

#[tokio::test]
async fn streamable_http_keeps_sessions_and_accepts_event_streams() {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  let server = tokio::spawn(async move {
    for index in 0..4 {
      let (mut socket, _) = listener.accept().await.unwrap();
      let request = String::from_utf8(read_http_request(&mut socket).await).unwrap();
      if index > 0 {
        assert!(
          request
            .to_ascii_lowercase()
            .contains("mcp-session-id: test-session")
        );
        assert!(
          request
            .to_ascii_lowercase()
            .contains("mcp-protocol-version: 2025-11-25")
        );
      }
      let (status, content_type, extra_headers, body) = match index {
        0 => (
          "200 OK",
          "application/json",
          "MCP-Session-Id: test-session\r\n",
          r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}"#.to_string(),
        ),
        1 => {
          assert!(request.contains("notifications/initialized"));
          ("202 Accepted", "application/json", "", String::new())
        }
        2 => (
          "200 OK",
          "text/event-stream",
          "",
          "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"Echo HTTP\",\"inputSchema\":{\"type\":\"object\"}}]}}\n\n".into(),
        ),
        _ => (
          "200 OK",
          "application/json",
          "",
          r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"http called"}]}}"#.to_string(),
        ),
      };
      let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
      );
      socket.write_all(response.as_bytes()).await.unwrap();
    }
  });
  let profile = McpProfile {
    servers: BTreeMap::from([(
      "remote".into(),
      McpServerConfig {
        transport: McpTransport::StreamableHttp,
        command: Vec::new(),
        url: Some(format!("http://{address}/mcp")),
        header_env: BTreeMap::new(),
        headers: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: None,
        enabled: true,
        required: false,
        timeout_ms: 1_000,
      },
    )]),
  };
  let temp = tempfile::tempdir().unwrap();
  let tool = Arc::new(McpHub::new(profile)).tool();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };
  let found = tool
    .execute(&context, json!({"command": "search", "query": "echo"}))
    .await
    .unwrap();
  assert_eq!(found, "remote/echo: Echo HTTP");
  let called = tool
    .execute(
      &context,
      json!({"command": "call", "server": "remote", "name": "echo"}),
    )
    .await
    .unwrap();
  assert_eq!(called, "http called");
  server.await.unwrap();
}

async fn read_http_request(socket: &mut TcpStream) -> Vec<u8> {
  let mut request = Vec::new();
  let mut buffer = [0_u8; 4096];
  loop {
    let read = socket.read(&mut buffer).await.unwrap();
    request.extend_from_slice(&buffer[..read]);
    let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
      continue;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let length = headers
      .lines()
      .find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name
          .eq_ignore_ascii_case("content-length")
          .then(|| value.trim().parse::<usize>().ok())
          .flatten()
      })
      .unwrap_or(0);
    if request.len() >= header_end + 4 + length {
      return request;
    }
  }
}

#[tokio::test]
async fn stdio_servers_skip_banners_and_restart_after_dying() {
  let temp = tempfile::tempdir().unwrap();
  let server = temp.path().join("server.sh");
  let counter = temp.path().join("starts");
  // the server prints a banner first, answers one call, then quits without warning
  tokio::fs::write(
    &server,
    format!(
      r#"#!/bin/sh
echo "starting up..."
printf 'x' >> "{}"
while IFS= read -r request; do
  id=$(printf '%s' "$request" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$request" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-11-25","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"flaky","version":"1"}}}}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"once","description":"Answer once","inputSchema":{{"type":"object"}}}}]}}}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"answered"}}]}}}}\n' "$id"
      exit 0
      ;;
  esac
done
"#,
      counter.display()
    ),
  )
  .await
  .unwrap();
  std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
  let profile = McpProfile {
    servers: BTreeMap::from([(
      "flaky".into(),
      McpServerConfig {
        transport: McpTransport::Stdio,
        command: vec![server.to_string_lossy().into_owned()],
        url: None,
        header_env: BTreeMap::new(),
        headers: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: None,
        enabled: true,
        required: true,
        timeout_ms: 2_000,
      },
    )]),
  };
  let tool = Arc::new(McpHub::new(profile)).tool();
  let context = ToolContext {
    workspace: temp.path().into(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };
  let call = json!({"command": "call", "server": "flaky", "name": "once"});
  assert_eq!(
    tool.execute(&context, call.clone()).await.unwrap(),
    "answered"
  );
  // once the process is gone the next call must start a fresh one rather than fail forever
  tokio::time::sleep(std::time::Duration::from_millis(300)).await;
  assert_eq!(tool.execute(&context, call).await.unwrap(), "answered");
  assert_eq!(
    tokio::fs::read_to_string(&counter).await.unwrap().len(),
    2,
    "expected exactly one restart"
  );
}

#[tokio::test]
async fn profiles_reject_names_that_cannot_be_addressed() {
  let temp = tempfile::tempdir().unwrap();
  let path = temp.path().join("launch.mcp.json");
  tokio::fs::write(
    &path,
    serde_json::to_vec(&json!({
      "mcpServers": {"bad name/here": {"command": "/bin/true"}}
    }))
    .unwrap(),
  )
  .await
  .unwrap();
  let error = McpProfile::load_with(Some(&path)).await.unwrap_err();
  assert!(error.to_string().contains("bad name/here"));
}
