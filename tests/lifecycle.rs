use std::{
  collections::{BTreeMap, VecDeque},
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};

use ainz::{
  Agent, ChatProvider, Event, EventSink, HookDef, HookRunner, PermissionMode, RunOptions, Session,
  protocol::{Message, Role, ToolCall, ToolSpec, Usage},
  provider::ProviderReply,
  tool::{ToolSet, builtins},
};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::json;

struct Provider(Mutex<VecDeque<Option<ProviderReply>>>);

#[async_trait]
impl ChatProvider for Provider {
  async fn complete(
    &self,
    messages: &[Message],
    tools: &[ToolSpec],
    _: &EventSink,
  ) -> Result<ProviderReply> {
    let next = self.0.lock().unwrap().pop_front();
    match next {
      Some(Some(reply)) => Ok(reply),
      Some(None) => {
        assert!(tools.is_empty(), "expected a compaction request");
        assert!(
          messages[0]
            .content
            .as_ref()
            .unwrap()
            .starts_with("Summarize")
        );
        std::future::pending().await
      }
      None => bail!("provider unavailable"),
    }
  }
}

fn reply(text: &str, tools: bool) -> Option<ProviderReply> {
  let mut message = Message::text(Role::Assistant, text);
  if tools {
    message.tool_calls = ["first", "second"]
      .into_iter()
      .map(|id| ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: json!({"path":"input.txt"}),
      })
      .collect();
  }
  Some(ProviderReply {
    message,
    usage: Usage {
      input_tokens: 7,
      output_tokens: 3,
      cost_usd: Some(0.25),
    },
  })
}

fn agent(
  root: &std::path::Path,
  replies: Vec<Option<ProviderReply>>,
  events: EventSink,
) -> Agent<Provider> {
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  Agent::new(
    Provider(Mutex::new(replies.into())),
    tools,
    root.into(),
    events,
    ainz::deny_all(),
  )
}

fn hook(root: &std::path::Path, event: &str) -> HookRunner {
  HookRunner::new(BTreeMap::from([(
    event.into(),
    vec![HookDef {
      command: vec![
        "/bin/sh".into(),
        "-c".into(),
        "cat >/dev/null; printf ready > \"$1\"; (sleep .4; printf survived > \"$2\") & wait".into(),
        "hook".into(),
        root.join("ready").display().to_string(),
        root.join("survived").display().to_string(),
      ],
      matcher: None,
    }],
  )]))
}

async fn wait_for(path: &std::path::Path) {
  tokio::time::timeout(Duration::from_secs(2), async {
    while !path.exists() {
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  })
  .await
  .expect("hook did not start");
}

#[tokio::test]
async fn cancellation_interrupts_every_hook_and_preserves_completed_tools() {
  for event in ["session_start", "pre_tool", "post_tool", "session_end"] {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::write(root.path().join("input.txt"), "read succeeded")
      .await
      .unwrap();
    let (events, mut rx) = EventSink::channel();
    let has_tools = matches!(event, "pre_tool" | "post_tool");
    let agent = agent(root.path(), vec![reply("answer", has_tools)], events);
    let mut session = Session::new(root.path().into());
    let (controller, mut inbox) = ainz::run_control();
    let options = RunOptions {
      hooks: hook(root.path(), event),
      ..Default::default()
    };
    let run = agent.run_controlled(&mut session, "hello".into(), options, &mut inbox);
    let cancel = async {
      wait_for(&root.path().join("ready")).await;
      let at = Instant::now();
      assert!(controller.cancel());
      at
    };
    let (result, at) =
      tokio::time::timeout(Duration::from_secs(3), async { tokio::join!(run, cancel) })
        .await
        .unwrap();
    assert!(
      result.unwrap_err().to_string().contains("run cancelled"),
      "{event}"
    );
    assert!(at.elapsed() < Duration::from_millis(250), "{event}");
    let mut cancelled = 0;
    while let Ok(event) = rx.try_recv() {
      assert!(!matches!(event, Event::TurnEnd { .. }));
      cancelled += usize::from(matches!(event, Event::Cancelled));
    }
    assert_eq!(cancelled, 1);
    let messages = session.messages().unwrap();
    if event == "session_start" {
      assert!(messages.is_empty());
      assert_eq!(session.usage.input_tokens, 0);
    } else {
      assert_eq!(session.usage.input_tokens, 7);
      assert_eq!(session.usage.cost_usd, Some(0.25));
    }
    if has_tools {
      let tools: Vec<_> = messages.iter().filter(|m| m.role == Role::Tool).collect();
      assert_eq!(tools.len(), 2);
      assert_eq!(tools[0].tool_call_id.as_deref(), Some("first"));
      assert_eq!(tools[1].tool_call_id.as_deref(), Some("second"));
      if event == "post_tool" {
        assert_eq!(tools[0].content.as_deref(), Some("read succeeded"));
      } else {
        assert!(tools[0].content.as_ref().unwrap().contains("cancelled"));
      }
      assert!(tools[1].content.as_ref().unwrap().contains("cancelled"));
    }
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(
      !root.path().join("survived").exists(),
      "{event} left descendants running"
    );
  }
}

#[tokio::test]
async fn usage_survives_a_later_provider_failure() {
  let root = tempfile::tempdir().unwrap();
  tokio::fs::write(root.path().join("input.txt"), "read succeeded")
    .await
    .unwrap();
  let agent = agent(
    root.path(),
    vec![reply("reading", true)],
    EventSink::default(),
  );
  let mut session = Session::new(root.path().into());
  session.usage.input_tokens = 5;
  session.usage.cost_usd = Some(0.25);
  let result = agent
    .run(&mut session, "hello".into(), RunOptions::default())
    .await;
  assert!(
    result
      .unwrap_err()
      .to_string()
      .contains("provider unavailable")
  );
  assert_eq!(session.usage.input_tokens, 12);
  assert_eq!(session.usage.output_tokens, 3);
  assert_eq!(session.usage.cost_usd, Some(0.5));
  assert_eq!(
    session
      .messages()
      .unwrap()
      .iter()
      .filter(|m| m.role == Role::Tool)
      .count(),
    2
  );
}

#[tokio::test]
async fn compaction_can_be_cancelled_without_losing_history() {
  let root = tempfile::tempdir().unwrap();
  let mut session = Session::new(root.path().into());
  for _ in 0..8 {
    session.append(Message::text(Role::User, "earlier work ".repeat(30)));
  }
  let original = session.nodes.len();
  let agent = agent(root.path(), vec![None], EventSink::default());
  let (controller, mut inbox) = ainz::run_control();
  let options = RunOptions {
    compact_at_tokens: 1,
    preserve_messages: 2,
    ..Default::default()
  };
  let run = agent.run_controlled(&mut session, "continue".into(), options, &mut inbox);
  let cancel = async {
    tokio::time::sleep(Duration::from_millis(20)).await;
    controller.cancel();
  };
  let (result, ()) = tokio::time::timeout(Duration::from_millis(250), async {
    tokio::join!(run, cancel)
  })
  .await
  .unwrap();
  assert!(result.unwrap_err().to_string().contains("run cancelled"));
  assert_eq!(session.nodes.len(), original + 1);
  assert!(session.summaries.is_empty());
}

#[tokio::test]
async fn an_empty_compaction_summary_still_records_reported_usage() {
  let root = tempfile::tempdir().unwrap();
  let agent = agent(root.path(), vec![reply("", false)], EventSink::default());
  let mut session = Session::new(root.path().into());
  for _ in 0..8 {
    session.append(Message::text(Role::User, "earlier work"));
  }
  let options = RunOptions {
    compact_at_tokens: 1,
    preserve_messages: 2,
    ..Default::default()
  };
  assert!(
    agent
      .run(&mut session, "continue".into(), options)
      .await
      .is_err()
  );
  assert_eq!(session.usage.input_tokens, 7);
  assert_eq!(session.usage.cost_usd, Some(0.25));
}

#[tokio::test]
async fn completion_waits_for_the_end_hook() {
  let root = tempfile::tempdir().unwrap();
  let marker = root.path().join("ended");
  let observed = Arc::new(Mutex::new(false));
  let received = observed.clone();
  let expected = marker.clone();
  let events = EventSink::new(move |event| {
    if matches!(event, Event::TurnEnd { .. }) {
      assert!(expected.exists());
      *received.lock().unwrap() = true;
    }
  });
  let agent = agent(root.path(), vec![reply("done", false)], events);
  let hooks = HookRunner::new(BTreeMap::from([(
    "session_end".into(),
    vec![HookDef {
      command: vec![
        "/bin/sh".into(),
        "-c".into(),
        "cat >/dev/null; touch \"$1\"".into(),
        "hook".into(),
        marker.display().to_string(),
      ],
      matcher: None,
    }],
  )]));
  let mut session = Session::new(root.path().into());
  let options = RunOptions {
    hooks,
    permissions: PermissionMode::Ask,
    ..Default::default()
  };
  assert_eq!(
    agent
      .run(&mut session, "hello".into(), options)
      .await
      .unwrap(),
    "done"
  );
  assert!(*observed.lock().unwrap());
  assert_eq!(session.usage.input_tokens, 7);
}
