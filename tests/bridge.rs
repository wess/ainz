//! A headless command that asks before it acts: its permission requests come back on stdout,
//! and the answer goes to it on stdin while it waits.

use std::sync::{Arc, Mutex};

use ainz::{
  Approver, EventSink, PermissionMode, ProcessOutput, ProcessProvider,
  protocol::{Message, Role, ToolCall},
  provider::ChatProvider,
  tool::Risk,
};

/// Reads the prompt, asks to run `ls`, and reports what it was told and the mode it was
/// started in. `cat` holds it open until its stdin closes, which is how it learns it may go.
const ASKING: &str = r#"
mode=; prev=
for arg in "$@"; do [ "$prev" = "--permission-mode" ] && mode=$arg; prev=$arg; done
read prompt
printf '%s\n' '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"t1"}}'
read answer
case "$answer" in *'"allow"'*) verdict=allowed;; *) verdict=denied;; esac
printf '{"type":"result","subtype":"success","is_error":false,"result":"%s %s"}\n' "$verdict" "$mode"
cat > /dev/null
"#;

fn asking(permissions: PermissionMode) -> ProcessProvider {
  ProcessProvider::new(
    "/bin/sh".into(),
    [
      "-c",
      ASKING,
      "sh",
      "--input-format",
      "stream-json",
      "--permission-mode",
      "{permission}",
      "--permission-prompt-tool",
      "stdio",
    ]
    .map(String::from)
    .to_vec(),
    "test".into(),
    std::env::current_dir().unwrap(),
    permissions,
    ProcessOutput::StreamJson,
  )
}

type Seen = Arc<Mutex<Vec<(ToolCall, Risk)>>>;

fn approver(answer: bool) -> (Approver, Seen) {
  let seen = Arc::new(Mutex::new(Vec::new()));
  let kept = seen.clone();
  let approver: Approver = Arc::new(move |call: &ToolCall, risk: Risk| {
    kept.lock().unwrap().push((call.clone(), risk));
    Box::pin(async move { answer })
  });
  (approver, seen)
}

async fn reply(provider: &ProcessProvider, mode: Option<(PermissionMode, &Approver)>) -> String {
  let message = Message::text(Role::User, "list the files");
  let events = EventSink::default();
  let reply = match mode {
    Some((mode, asks)) => {
      provider
        .complete_asking(&[&message], &[], &events, mode, asks)
        .await
    }
    None => provider.complete(&[&message], &[], &events).await,
  };
  reply.unwrap().message.content.unwrap()
}

#[tokio::test]
async fn a_headless_command_asks_the_person_and_runs_what_they_allow() {
  let provider = asking(PermissionMode::Ask);
  let (asks, seen) = approver(true);

  let answer = reply(&provider, Some((PermissionMode::Ask, &asks))).await;

  // a command that can ask runs in its own default mode rather than planning
  assert_eq!(answer, "allowed default");
  let seen = seen.lock().unwrap();
  assert_eq!(seen[0].0.name, "shell");
  assert_eq!(seen[0].0.arguments["command"], "ls");
  assert_eq!(seen[0].1, Risk::Execute);
}

#[tokio::test]
async fn a_call_the_person_refuses_is_denied() {
  let provider = asking(PermissionMode::Ask);
  let (asks, _) = approver(false);

  assert_eq!(
    reply(&provider, Some((PermissionMode::Ask, &asks))).await,
    "denied default"
  );
}

#[tokio::test]
async fn a_command_asked_outside_a_run_is_refused_rather_than_left_waiting() {
  let provider = asking(PermissionMode::Ask);

  assert_eq!(reply(&provider, None).await, "denied default");
}

#[tokio::test]
async fn read_only_still_plans() {
  let provider = asking(PermissionMode::ReadOnly);
  let (asks, _) = approver(true);

  assert_eq!(
    reply(&provider, Some((PermissionMode::ReadOnly, &asks))).await,
    "allowed plan"
  );
}

/// The real `claude`, asked to run one command. Ignored by default: it needs the CLI signed in
/// and spends a request. `cargo test --test bridge -- --ignored` runs it.
async fn real(answer: bool) -> (String, Vec<(ToolCall, Risk)>, bool) {
  let workspace = tempfile::tempdir().unwrap();
  // the person's own Claude settings may allow Bash outright, and a call they allow never
  // reaches the bridge; only the project's settings, of which a scratch directory has none
  let mut args = ainz::config::ProviderConfig::claude_code().args;
  args.extend(["--setting-sources".into(), "project,local".into()]);
  let provider = ProcessProvider::new(
    "claude".into(),
    args,
    "haiku".into(),
    workspace.path().into(),
    PermissionMode::Ask,
    ProcessOutput::StreamJson,
  );
  let (asks, seen) = approver(answer);
  let prompt = Message::text(
    Role::User,
    "Use the Bash tool to run exactly this command, once: touch made-by-bridge.txt. Then reply \
     DONE if it ran, or DENIED if you were not allowed to run it.",
  );
  let reply = provider
    .complete_asking(
      &[&prompt],
      &[],
      &EventSink::default(),
      PermissionMode::Ask,
      &asks,
    )
    .await
    .unwrap();
  let seen = seen.lock().unwrap().clone();
  let made = workspace.path().join("made-by-bridge.txt").exists();
  (reply.message.content.unwrap_or_default(), seen, made)
}

// a plain `echo` is on Claude's own list of commands it never asks about, so the probe writes
fn asked_to_touch(seen: &[(ToolCall, Risk)]) -> bool {
  seen.iter().any(|(call, risk)| {
    call.name == "shell"
      && *risk == Risk::Execute
      && call.arguments["command"]
        .as_str()
        .is_some_and(|command| command.contains("touch made-by-bridge.txt"))
  })
}

#[tokio::test]
#[ignore]
async fn the_real_claude_asks_and_runs_what_is_allowed() {
  let (reply, seen, made) = real(true).await;
  assert!(asked_to_touch(&seen), "{seen:?}");
  assert!(made, "{reply}");
}

#[tokio::test]
#[ignore]
async fn the_real_claude_is_told_no_and_carries_on() {
  let (reply, seen, made) = real(false).await;
  assert!(asked_to_touch(&seen), "{seen:?}");
  assert!(!made, "{reply}");
  assert!(reply.to_ascii_uppercase().contains("DENIED"), "{reply}");
}
