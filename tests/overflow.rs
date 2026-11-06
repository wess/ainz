//! A conversation against a provider with a real window. It refuses whatever is too long the way
//! a model does, a summary request included, and it counts three bytes to the token where the
//! estimate counts four, which is what a paste of JSON does to an estimate.

use std::sync::{Arc, Mutex};

use ainz::{
  Agent, EventSink, HookRunner, PermissionMode, RunOptions, Session,
  protocol::{Message, Role, ToolCall, ToolSpec, Usage},
  provider::{ChatProvider, ProviderReply},
  tool::ToolSet,
};
use async_trait::async_trait;

const WINDOW: usize = 16_000;
const BYTES_PER_TOKEN: usize = 3;

/// More than the window however it is counted.
const OVERSIZED: usize = 300_000;

/// Each fits alone, and no two fit together.
const MOST_OF_THE_WINDOW: usize = 33_000;

/// Inside the estimate and over the window, so only the provider can say it does not fit.
const UNDERESTIMATED: usize = 50_000;

#[derive(Clone, Default)]
struct Window(Arc<Mutex<Vec<usize>>>);

impl Window {
  fn requests(&self) -> Vec<usize> {
    self.0.lock().unwrap().clone()
  }
}

#[async_trait]
impl ChatProvider for Window {
  async fn complete(
    &self,
    messages: &[&Message],
    _tools: &[ToolSpec],
    _events: &EventSink,
  ) -> anyhow::Result<ProviderReply> {
    let bytes = messages
      .iter()
      .map(|message| message.content.as_ref().map_or(0, String::len))
      .sum::<usize>();
    self.0.lock().unwrap().push(bytes);
    let tokens = bytes / BYTES_PER_TOKEN;
    if tokens > WINDOW {
      anyhow::bail!("400 Bad Request: prompt is too long: {tokens} tokens > {WINDOW} maximum");
    }
    let summarising = messages[0]
      .content
      .as_deref()
      .is_some_and(|text| text.starts_with("Summarize the session"));
    Ok(ProviderReply {
      message: Message::text(
        Role::Assistant,
        if summarising {
          "the summary"
        } else {
          "answered"
        },
      ),
      usage: Usage::default(),
    })
  }
}

fn options() -> RunOptions {
  RunOptions {
    instructions: "test".into(),
    permissions: PermissionMode::Auto,
    max_steps: 4,
    max_output_bytes: 1024,
    context_tokens: WINDOW,
    compact_at_tokens: 12_000,
    preserve_messages: 8,
    memory_nudge: None,
    budget_usd: None,
    rules: Default::default(),
    hooks: HookRunner::default(),
  }
}

fn agent(window: &Window) -> Agent<Window> {
  Agent::new(
    window.clone(),
    ToolSet::default(),
    std::env::temp_dir(),
    EventSink::default(),
    ainz::deny_all(),
  )
}

fn session() -> Session {
  Session::new(std::env::temp_dir())
}

fn exchange(session: &mut Session, prompt: &str, reply: &str) {
  session.append(Message::text(Role::User, prompt));
  session.append(Message::text(Role::Assistant, reply));
}

#[tokio::test]
async fn a_message_too_big_for_the_window_is_refused_and_the_conversation_carries_on() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  exchange(&mut session, "the parser", "it is in src/parser");
  let error = agent
    .run(&mut session, "x".repeat(OVERSIZED), options())
    .await
    .unwrap_err();
  assert!(format!("{error:#}").contains("not sent"), "{error:#}");
  assert!(
    window.requests().is_empty(),
    "nothing can help a message that does not fit, so nothing is spent trying"
  );
  assert_eq!(session.messages().unwrap().len(), 2);

  let answer = agent
    .run(&mut session, "and now?".into(), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
  assert!(session.summaries.is_empty(), "the history was not touched");
}

#[tokio::test]
async fn a_large_message_the_prompt_cannot_share_the_window_with_is_archived() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  exchange(&mut session, &"x".repeat(MOST_OF_THE_WINDOW), "read it");
  let answer = agent
    .run(&mut session, "y".repeat(MOST_OF_THE_WINDOW), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
}

#[tokio::test]
async fn results_too_big_to_send_do_not_outlive_the_run_they_ended() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  session.append(Message::text(Role::User, "read it all"));
  session.append(Message::assistant(
    None,
    None,
    vec![ToolCall {
      id: "call-1".into(),
      name: "read".into(),
      arguments: serde_json::json!({}),
    }],
  ));
  session.append(Message::tool("call-1", "x".repeat(OVERSIZED)));
  let answer = agent
    .run(&mut session, "go on".into(), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
}

#[tokio::test]
async fn a_session_saved_with_an_oversized_prompt_recovers_on_its_next_prompt() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  exchange(&mut session, "the parser", "it is in src/parser");
  session.append(Message::text(Role::User, "x".repeat(OVERSIZED)));
  let answer = agent
    .run(&mut session, "and now?".into(), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
  assert!(
    *window.requests().last().unwrap() < OVERSIZED / 100,
    "the paste went out again"
  );
}

#[tokio::test]
async fn an_oversized_message_long_past_is_summarised_without_being_sent_whole() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  exchange(&mut session, &"x".repeat(OVERSIZED), "read it");
  for index in 0..8 {
    exchange(&mut session, &format!("question {index}"), "an answer");
  }
  let answer = agent
    .run(&mut session, "and now?".into(), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
  assert!(
    window
      .requests()
      .iter()
      .all(|bytes| bytes / BYTES_PER_TOKEN <= WINDOW),
    "{:?}",
    window.requests()
  );
}

#[tokio::test]
async fn a_long_history_is_summarised_in_pieces_that_each_fit() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  for index in 0..60 {
    exchange(
      &mut session,
      &format!("question {index} {}", "x".repeat(3_000)),
      &format!("answer {index} {}", "y".repeat(3_000)),
    );
  }
  let answer = agent
    .run(&mut session, "and now?".into(), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
  let requests = window.requests();
  assert!(requests.len() > 2, "{requests:?}");
  assert!(
    requests
      .iter()
      .all(|bytes| bytes / BYTES_PER_TOKEN <= WINDOW),
    "{requests:?}"
  );
}

#[tokio::test]
async fn a_refusal_the_kept_tail_cannot_answer_is_answered_by_keeping_only_the_prompt() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  exchange(&mut session, &"x".repeat(UNDERESTIMATED), "read it");
  let answer = agent
    .run(&mut session, "go on".into(), options())
    .await
    .unwrap();
  assert_eq!(answer, "answered");
  assert_eq!(session.summaries.len(), 1);
}

#[tokio::test]
async fn a_refusal_with_nothing_left_to_give_up_ends_the_run_with_the_provider_s_words() {
  let window = Window::default();
  let agent = agent(&window);
  let mut session = session();
  let error = agent
    .run(&mut session, "x".repeat(UNDERESTIMATED), options())
    .await
    .unwrap_err();
  assert!(
    format!("{error:#}").contains("prompt is too long"),
    "{error:#}"
  );
}
