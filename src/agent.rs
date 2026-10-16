use std::{future::Future, panic::AssertUnwindSafe, path::PathBuf, pin::Pin, sync::Arc};

use anyhow::{Result, anyhow, bail};
use futures_util::FutureExt;
use serde_json::Value;

use crate::{
  context::{message_bytes, tool_bytes},
  control::RunInbox,
  event::{Event, EventSink},
  protocol::{Image, Message, Role, ToolCall, ToolSpec, Usage},
  provider::ChatProvider,
  session::{Context, Session},
  tool::{Risk, ToolSet},
};

pub type Approval = Pin<Box<dyn Future<Output = bool> + Send>>;
pub type Approver = Arc<dyn Fn(&ToolCall, Risk) -> Approval + Send + Sync>;

// for surfaces with nobody to ask: json output, rpc, tests
pub fn deny_all() -> Approver {
  Arc::new(|_, _| Box::pin(async { false }))
}

mod context;
mod lifecycle;
mod options;
mod plan;
mod step;
mod tool;

use lifecycle::RunState;
use step::{Action, Effect, Turn, step};

pub use options::RunOptions;

pub struct Agent<P> {
  provider: P,
  tools: ToolSet,
  workspace: PathBuf,
  events: EventSink,
  approver: Approver,
}

impl<P: ChatProvider> Agent<P> {
  pub fn new(
    provider: P,
    tools: ToolSet,
    workspace: PathBuf,
    events: EventSink,
    approver: Approver,
  ) -> Self {
    Self {
      provider,
      tools,
      workspace,
      events,
      approver,
    }
  }

  pub async fn run(
    &self,
    session: &mut Session,
    prompt: String,
    options: RunOptions,
  ) -> Result<String> {
    self
      .run_message(session, Message::text(Role::User, prompt), options, None)
      .await
  }

  pub async fn run_with_images(
    &self,
    session: &mut Session,
    prompt: String,
    images: Vec<Image>,
    options: RunOptions,
  ) -> Result<String> {
    self
      .run_message(session, Message::user(prompt, images), options, None)
      .await
  }

  pub async fn run_controlled(
    &self,
    session: &mut Session,
    prompt: String,
    options: RunOptions,
    inbox: &mut RunInbox,
  ) -> Result<String> {
    self
      .run_message(
        session,
        Message::text(Role::User, prompt),
        options,
        Some(inbox),
      )
      .await
  }

  pub async fn run_controlled_with_images(
    &self,
    session: &mut Session,
    prompt: String,
    images: Vec<Image>,
    options: RunOptions,
    inbox: &mut RunInbox,
  ) -> Result<String> {
    self
      .run_message(session, Message::user(prompt, images), options, Some(inbox))
      .await
  }

  async fn run_turn(
    &self,
    session: &mut Session,
    prompt: Message,
    options: &RunOptions,
    mut inbox: Option<&mut RunInbox>,
    state: &mut RunState,
  ) -> Result<String> {
    session.append(prompt);
    let specs = self.tools.specs();
    // neither changes during a run, so neither is measured or rebuilt on every step
    let fixed = options.instructions.len() + tool_bytes(&specs);
    let system = Message::text(Role::System, &options.instructions);
    state.planning = self.tools.get(crate::todo::TOOL).is_some();
    if state.planning {
      state.plan = plan::recalled(session.context()?.messages);
    }
    let mut turn = Turn::new(options.max_steps);
    let mut final_text = String::new();
    let mut effect = step(&mut turn, Action::Begin);
    loop {
      state.checkpoint()?;
      effect = match effect {
        Effect::Ask => {
          let reply = self
            .ask(session, options, &system, &specs, fixed, state)
            .await?;
          final_text = reply.content.clone().unwrap_or_default();
          let calls = reply.tool_calls.len();
          state.pending = reply.tool_calls.iter().cloned().collect();
          session.append(reply);
          step(&mut turn, Action::Replied { calls })
        }
        Effect::RunTools => {
          self.run_tools(session, options, state).await?;
          step(&mut turn, Action::ToolsDone)
        }
        Effect::Drain => {
          let steering = inbox
            .as_deref_mut()
            .map(|inbox| inbox.drain(turn.settled))
            .unwrap_or_default();
          let messages = steering.len();
          for message in steering {
            self.events.emit(Event::Steering {
              message: message.clone(),
            });
            session.append(Message::text(Role::User, message));
          }
          step(&mut turn, Action::Steered { messages })
        }
        Effect::Finish => return Ok(final_text),
        Effect::Pause => {
          self.events.emit(Event::Notice {
            message: format!(
              "paused after {} steps with work still open; send a message to carry on",
              turn.steps
            ),
          });
          return Ok(final_text);
        }
      };
    }
  }

  async fn ask(
    &self,
    session: &mut Session,
    options: &RunOptions,
    system: &Message,
    specs: &[ToolSpec],
    fixed: usize,
    state: &mut RunState,
  ) -> Result<Message> {
    let full = tokens(fixed, &session.context()?) >= options.compact_at_tokens;
    if full {
      self.compact(session, options, &mut state.usage).await?;
    }
    let context = session.context()?;
    let estimate = tokens(fixed, &context);
    if estimate > options.context_tokens {
      bail!(
        "estimated context is {estimate} tokens, above the {} token limit",
        options.context_tokens
      );
    }
    let mut messages = Vec::with_capacity(context.len() + 1);
    messages.push(system);
    messages.extend(context.iter());
    state.checkpoint()?;
    // a provider that panics fails the run cleanly; the session it was handed is still whole
    let reply = AssertUnwindSafe(self.provider.complete(&messages, specs, &self.events))
      .catch_unwind()
      .await
      .map_err(|panic| anyhow!("the provider failed: {}", tool::panic_message(&*panic)))??;
    accumulate(&mut state.usage, &reply.usage);
    Ok(reply.message)
  }
}

fn tokens(fixed: usize, context: &Context<'_>) -> usize {
  (fixed + context.iter().map(message_bytes).sum::<usize>()).div_ceil(4)
}

/// The part of a call a rule is written against: the command, the path, whatever names what
/// the call acts on. The transcript labels a call with the same field.
pub fn subject(arguments: &Value) -> Option<&str> {
  const SUBJECT: [&str; 6] = ["command", "path", "file_path", "pattern", "url", "query"];
  SUBJECT
    .iter()
    .find_map(|key| arguments.get(key).and_then(Value::as_str))
}

fn accumulate(total: &mut Usage, usage: &Usage) {
  total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
  total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
  total.cost_usd = add_cost(total.cost_usd, usage.cost_usd);
}

// a cost nobody reported is not zero, it is unknown, so it stays None until one is
fn add_cost(total: Option<f64>, next: Option<f64>) -> Option<f64> {
  match (total, next) {
    (Some(total), Some(next)) => Some(total + next),
    (total, next) => total.or(next),
  }
}
