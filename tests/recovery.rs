//! How a run gets past a reply that did not finish: one cut off at its output limit carries on,
//! one that broke off partway is asked for again, and what the provider counted becomes the
//! estimate the next request is sized by.

use std::sync::Mutex;

use ainz::{
  Agent, Event, EventSink, HookRunner, PermissionMode, RunOptions, Session,
  protocol::{Message, Role, ToolCall, ToolSpec, Usage},
  provider::{ChatProvider, ProviderReply},
  tool::{ToolSet, builtins},
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::json;

fn options() -> RunOptions {
  RunOptions {
    instructions: "test".into(),
    permissions: PermissionMode::Auto,
    max_steps: 16,
    max_output_bytes: 1024,
    context_tokens: 16_000,
    compact_at_tokens: 12_000,
    preserve_messages: 4,
    memory_nudge: None,
    budget_usd: None,
    rules: Default::default(),
    hooks: HookRunner::default(),
  }
}

type Step = Box<dyn Fn() -> Result<ProviderReply> + Send + Sync>;

/// Answers each request with the next step, and keeps the system prompt each one was sent.
struct Script {
  steps: Mutex<Vec<Step>>,
  systems: Mutex<Vec<String>>,
  runs_tools: bool,
}

impl Script {
  fn new(steps: Vec<Step>) -> Self {
    Self {
      steps: Mutex::new(steps.into_iter().rev().collect()),
      systems: Mutex::new(Vec::new()),
      runs_tools: false,
    }
  }

  fn requests(&self) -> usize {
    self.systems.lock().unwrap().len()
  }
}

#[async_trait]
impl ChatProvider for Script {
  async fn complete(
    &self,
    messages: &[&Message],
    _tools: &[ToolSpec],
    _events: &EventSink,
  ) -> Result<ProviderReply> {
    let system = messages
      .iter()
      .find(|message| message.role == Role::System)
      .and_then(|message| message.content.clone())
      .unwrap_or_default();
    self.systems.lock().unwrap().push(system);
    let step = self.steps.lock().unwrap().pop().expect("no step left");
    step()
  }

  fn runs_tools(&self) -> bool {
    self.runs_tools
  }
}

fn text(content: &str, truncated: bool) -> Step {
  let content = content.to_string();
  Box::new(move || {
    Ok(ProviderReply {
      message: Message::text(Role::Assistant, content.clone()),
      usage: Usage::default(),
      truncated,
    })
  })
}

fn write(path: &str) -> Step {
  let path = path.to_string();
  Box::new(move || {
    Ok(ProviderReply {
      message: Message::assistant(
        None,
        None,
        vec![ToolCall {
          id: format!("call-{path}"),
          name: "write".into(),
          arguments: json!({"path": path, "content": "x"}),
        }],
      ),
      usage: Usage::default(),
      truncated: true,
    })
  })
}

fn failing(error: &'static str) -> Step {
  Box::new(move || Err(anyhow!(error)))
}

fn counted(input_tokens: u64) -> Step {
  Box::new(move || {
    Ok(ProviderReply {
      message: Message::text(Role::Assistant, "done"),
      usage: Usage {
        input_tokens,
        output_tokens: 1,
        cost_usd: None,
      },
      truncated: false,
    })
  })
}

fn agent(provider: Script, events: EventSink) -> (Agent<Script>, tempfile::TempDir) {
  let temp = tempfile::tempdir().unwrap();
  let mut tools = ToolSet::default();
  tools.extend(builtins()).unwrap();
  let agent = Agent::new(
    provider,
    tools,
    temp.path().into(),
    events,
    ainz::deny_all(),
  );
  (agent, temp)
}

fn notices(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) -> Vec<String> {
  let mut notices = Vec::new();
  while let Ok(event) = events.try_recv() {
    if let Event::Notice { message } = event {
      notices.push(message);
    }
  }
  notices
}

#[tokio::test]
async fn a_text_reply_cut_off_by_the_output_limit_continues_into_the_requested_work() {
  let (events, mut received) = EventSink::channel();
  let (agent, temp) = agent(
    Script::new(vec![text("first half", true), text("second half", false)]),
    events,
  );
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "explain".into(), options())
    .await
    .unwrap();

  assert_eq!(output, "second half");
  let systems = agent_systems(&agent);
  assert!(!systems[0].contains("output limit"));
  assert!(
    systems[1].contains("reached its output limit"),
    "{}",
    systems[1]
  );
  assert!(
    notices(&mut received)
      .iter()
      .any(|notice| notice.contains("continuing automatically"))
  );
}

#[tokio::test]
async fn repeated_cutoffs_pause_after_three_continuations_without_tool_progress() {
  let (events, mut received) = EventSink::channel();
  let steps = (0..4).map(|_| text("again", true)).collect();
  let (agent, temp) = agent(Script::new(steps), events);
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "explain".into(), options())
    .await
    .unwrap();

  assert!(output.starts_with("again"));
  assert_eq!(agent_requests(&agent), 4);
  assert!(
    notices(&mut received)
      .iter()
      .any(|notice| notice.contains("paused after 3 automatic continuations"))
  );
}

#[tokio::test]
async fn successful_tool_work_resets_the_consecutive_cutoff_limit() {
  let (events, _received) = EventSink::channel();
  let steps = vec![
    text("a", true),
    text("b", true),
    write("one.txt"),
    text("c", true),
    text("d", true),
    text("done", false),
  ];
  let (agent, temp) = agent(Script::new(steps), events);
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  assert_eq!(output, "done");
  assert!(temp.path().join("one.txt").exists());
}

#[tokio::test(start_paused = true)]
async fn a_reply_that_broke_off_is_asked_for_again() {
  let (events, mut received) = EventSink::channel();
  let (agent, temp) = agent(
    Script::new(vec![
      failing("read model stream: connection reset by peer"),
      text("whole", false),
    ]),
    events,
  );
  let mut session = Session::new(temp.path().into());

  let output = agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  assert_eq!(output, "whole");
  assert!(
    notices(&mut received)
      .iter()
      .any(|notice| notice.contains("broke off"))
  );
}

#[tokio::test(start_paused = true)]
async fn a_reply_that_keeps_breaking_off_ends_the_run_with_its_error() {
  let (agent, temp) = agent(
    Script::new(vec![
      failing("model stream ended before completion"),
      failing("model stream ended before completion"),
      failing("model stream ended before completion"),
    ]),
    EventSink::default(),
  );
  let mut session = Session::new(temp.path().into());

  let error = agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap_err();

  assert!(error.to_string().contains("ended before completion"));
  assert_eq!(agent_requests(&agent), 3);
}

#[tokio::test]
async fn a_command_that_runs_its_own_tools_is_not_asked_twice() {
  let mut script = Script::new(vec![failing("model stream ended before completion")]);
  script.runs_tools = true;
  let (agent, temp) = agent(script, EventSink::default());
  let mut session = Session::new(temp.path().into());

  assert!(
    agent
      .run(&mut session, "go".into(), options())
      .await
      .is_err()
  );
  assert_eq!(agent_requests(&agent), 1);
}

#[tokio::test]
async fn the_context_used_is_what_the_provider_counted() {
  let (events, mut received) = EventSink::channel();
  let (agent, temp) = agent(Script::new(vec![counted(4_321)]), events);
  let mut session = Session::new(temp.path().into());

  agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  let mut context = None;
  while let Ok(event) = received.try_recv() {
    if let Event::Context { used, limit } = event {
      context = Some((used, limit));
    }
  }
  assert_eq!(context, Some((4_321, 16_000)));
}

#[tokio::test]
async fn what_the_provider_counts_decides_when_to_compact() {
  // the first request is a few kilobytes, which the provider says came to far more tokens than
  // four bytes each; the next prompt is then measured by that and refused, where the byte
  // guess alone would have let it through
  let (agent, temp) = agent(Script::new(vec![counted(10_000)]), EventSink::default());
  let mut session = Session::new(temp.path().into());
  agent
    .run(&mut session, "go".into(), options())
    .await
    .unwrap();

  let prompt = "word ".repeat(9_000);
  let error = agent
    .run(&mut session, prompt, options())
    .await
    .unwrap_err();
  assert!(error.to_string().contains("not sent"), "{error}");
}

fn agent_systems(agent: &Agent<Script>) -> Vec<String> {
  agent.provider().systems.lock().unwrap().clone()
}

fn agent_requests(agent: &Agent<Script>) -> usize {
  agent.provider().requests()
}
