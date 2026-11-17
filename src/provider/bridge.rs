//! Claude Code's permission requests, answered by Ainz. With `--permission-prompt-tool stdio`
//! and `--input-format stream-json`, `claude -p` writes a `can_use_tool` request where it would
//! otherwise show its own prompt, and waits on stdin for the answer. Without them a headless
//! run has nobody to ask, and refuses the call without telling anyone it wanted to make it.

use serde_json::{Value, json};

use crate::{agent::Approver, protocol::ToolCall, tool::Risk};

/// Whether these arguments send the command's permission requests back to Ainz.
pub(super) fn bridged(args: &[String]) -> bool {
  args
    .iter()
    .any(|arg| arg == "--permission-prompt-tool=stdio")
    || args
      .windows(2)
      .any(|pair| pair[0] == "--permission-prompt-tool" && pair[1] == "stdio")
}

/// The line that answers one request the command is waiting on, or nothing for a request with
/// no id to answer. Only a tool's permission is Ainz's to give; anything else is refused rather
/// than ignored, so the command is not left waiting.
pub(super) async fn answer(request: &Value, asks: &Approver) -> Option<String> {
  let id = request.get("request_id").and_then(Value::as_str)?;
  let body = &request["request"];
  let subtype = body.get("subtype").and_then(Value::as_str).unwrap_or("");
  if subtype != "can_use_tool" {
    return Some(line(json!({
      "subtype": "error",
      "request_id": id,
      "error": format!("ainz does not answer {subtype} requests"),
    })));
  }
  let name = body.get("tool_name").and_then(Value::as_str).unwrap_or("");
  let input = body.get("input").cloned().unwrap_or_else(|| json!({}));
  let use_id = body
    .get("tool_use_id")
    .and_then(Value::as_str)
    .unwrap_or(id);
  let (call, risk) = call(name, &input, use_id);
  let decision = if asks(&call, risk).await {
    json!({ "behavior": "allow", "updatedInput": input })
  } else {
    json!({
      "behavior": "deny",
      "message": format!(
        "permission denied for {name} ({risk:?}). Do not retry it; ask the person what to do \
         instead."
      ),
    })
  };
  Some(line(json!({
    "subtype": "success",
    "request_id": id,
    "response": decision,
  })))
}

fn line(response: Value) -> String {
  let message = json!({ "type": "control_response", "response": response });
  format!("{message}\n")
}

/// The call as Ainz's rules and approval prompt know it. Claude Code capitalises its tool names
/// and calls a path `file_path`, so without this a rule such as `shell(git status*)`, or the one
/// a person sets with "always", would never match the call it was written for.
fn call(name: &str, input: &Value, id: &str) -> (ToolCall, Risk) {
  let mut arguments = input.clone();
  let mcp = name
    .strip_prefix("mcp__")
    .and_then(|rest| rest.split_once("__"));
  let ainz = match (name, mcp) {
    (_, Some(_)) => "mcp",
    ("Bash", _) => "shell",
    ("Read", _) => "read",
    ("LS", _) => "list",
    ("Glob" | "Grep", _) => "search",
    ("Write", _) => "write",
    ("Edit" | "MultiEdit" | "NotebookEdit", _) => "edit",
    ("WebFetch" | "WebSearch", _) => "fetch",
    (other, None) => return (named(other, arguments, id), Risk::Execute),
  };
  if let Some(object) = arguments.as_object_mut() {
    if let Some((server, tool)) = mcp {
      object.insert("name".into(), json!(format!("{server}/{tool}")));
    }
    let path = ["file_path", "notebook_path"]
      .iter()
      .find_map(|key| object.get(*key).cloned());
    if let Some(path) = path
      && !object.contains_key("path")
    {
      object.insert("path".into(), path);
    }
  }
  let risk = match ainz {
    "read" | "list" | "search" => Risk::Read,
    "write" | "edit" => Risk::Write,
    "fetch" => Risk::Network,
    _ => Risk::Execute,
  };
  (named(ainz, arguments, id), risk)
}

fn named(name: &str, arguments: Value, id: &str) -> ToolCall {
  ToolCall {
    id: id.to_string(),
    name: name.to_ascii_lowercase(),
    arguments,
  }
}

#[cfg(test)]
mod requests {
  use std::sync::{Arc, Mutex};

  use super::*;

  type Asked = Arc<Mutex<Vec<(ToolCall, Risk)>>>;

  fn asked(answer: bool) -> (Approver, Asked) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    let asks: Approver = Arc::new(move |call: &ToolCall, risk: Risk| {
      kept.lock().unwrap().push((call.clone(), risk));
      Box::pin(async move { answer })
    });
    (asks, seen)
  }

  fn request(tool: &str, input: Value) -> Value {
    json!({
      "type": "control_request",
      "request_id": "r1",
      "request": {
        "subtype": "can_use_tool",
        "tool_name": tool,
        "input": input,
        "tool_use_id": "toolu_1",
      },
    })
  }

  fn reply(line: &str) -> Value {
    serde_json::from_str(line.trim()).unwrap()
  }

  #[test]
  fn only_the_stdio_prompt_tool_is_a_bridge() {
    let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    assert!(bridged(&args(&["-p", "--permission-prompt-tool", "stdio"])));
    assert!(bridged(&args(&["--permission-prompt-tool=stdio"])));
    assert!(!bridged(&args(&[
      "--permission-prompt-tool",
      "mcp__x__ask"
    ])));
    assert!(!bridged(&args(&[
      "-p",
      "--permission-mode",
      "{permission}"
    ])));
  }

  #[tokio::test]
  async fn an_allowed_call_runs_with_the_input_the_command_sent() {
    let (asks, seen) = asked(true);
    let input = json!({"command": "cargo test", "description": "run the tests"});
    let line = answer(&request("Bash", input.clone()), &asks)
      .await
      .unwrap();
    let reply = reply(&line);
    assert_eq!(reply["type"], "control_response");
    assert_eq!(reply["response"]["request_id"], "r1");
    assert_eq!(reply["response"]["response"]["behavior"], "allow");
    assert_eq!(reply["response"]["response"]["updatedInput"], input);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0.name, "shell");
    assert_eq!(seen[0].1, Risk::Execute);
  }

  #[tokio::test]
  async fn a_refused_call_is_denied_in_words() {
    let (asks, _) = asked(false);
    let line = answer(&request("Bash", json!({"command": "rm -rf build"})), &asks)
      .await
      .unwrap();
    let reply = reply(&line);
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    assert!(
      reply["response"]["response"]["message"]
        .as_str()
        .unwrap()
        .contains("permission denied for Bash")
    );
  }

  #[tokio::test]
  async fn a_claude_code_call_reaches_the_rules_under_ainz_names() {
    let (asks, seen) = asked(true);
    for (tool, input) in [
      (
        "Edit",
        json!({"file_path": "src/lib.rs", "old_string": "a", "new_string": "b"}),
      ),
      (
        "WebFetch",
        json!({"url": "https://docs.rs/serde", "prompt": "read"}),
      ),
      ("mcp__outline__search", json!({"query": "plans"})),
      ("Task", json!({"prompt": "look around"})),
    ] {
      answer(&request(tool, input), &asks).await.unwrap();
    }
    let seen = seen.lock().unwrap();
    assert_eq!((seen[0].0.name.as_str(), seen[0].1), ("edit", Risk::Write));
    assert_eq!(seen[0].0.arguments["path"], "src/lib.rs");
    assert_eq!(
      (seen[1].0.name.as_str(), seen[1].1),
      ("fetch", Risk::Network)
    );
    assert_eq!(seen[2].0.name, "mcp");
    assert_eq!(seen[2].0.arguments["name"], "outline/search");
    assert_eq!(
      (seen[3].0.name.as_str(), seen[3].1),
      ("task", Risk::Execute)
    );
  }

  #[tokio::test]
  async fn a_request_that_is_not_about_a_tool_is_refused_rather_than_left_waiting() {
    let (asks, seen) = asked(true);
    let request = json!({
      "type": "control_request",
      "request_id": "r2",
      "request": { "subtype": "hook_callback" },
    });
    let reply = reply(&answer(&request, &asks).await.unwrap());
    assert_eq!(reply["response"]["subtype"], "error");
    assert_eq!(reply["response"]["request_id"], "r2");
    assert!(seen.lock().unwrap().is_empty());
  }
}
