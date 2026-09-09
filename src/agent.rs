use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::{
  context::estimate_tokens,
  control::RunInbox,
  event::{Event, EventSink},
  protocol::{Image, Message, Role, ToolCall, Usage},
  provider::ChatProvider,
  session::Session,
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
mod tool;

use lifecycle::RunState;

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
    for _ in 0..options.max_steps {
      state.checkpoint()?;
      let mut context_messages = session.context_messages()?;
      let mut estimate = estimate_tokens(&options.instructions, &context_messages, &specs);
      if estimate >= options.compact_at_tokens
        && self.compact(session, options, &mut state.usage).await?
      {
        context_messages = session.context_messages()?;
        estimate = estimate_tokens(&options.instructions, &context_messages, &specs);
      }
      if estimate > options.context_tokens {
        bail!(
          "estimated context is {estimate} tokens, above the {} token limit",
          options.context_tokens
        );
      }
      let mut messages = vec![Message::text(Role::System, &options.instructions)];
      messages.extend(context_messages);
      state.checkpoint()?;
      let reply = self
        .provider
        .complete(&messages, &specs, &self.events)
        .await?;
      accumulate(&mut state.usage, &reply.usage);
      let final_text = reply.message.content.clone().unwrap_or_default();
      let final_reply = reply.message.tool_calls.is_empty();
      state.pending = reply.message.tool_calls.clone().into();
      session.append(reply.message);
      while let Some(call) = state.pending.front().cloned() {
        self.events.emit(Event::ToolStart { call: call.clone() });
        state.checkpoint()?;
        let (output, error, executed) = self.run_tool(&call, options, session.id).await;
        session.append(Message::tool(call.id.clone(), output.clone()));
        state.pending.pop_front();
        self.events.emit(Event::ToolEnd {
          id: call.id.clone(),
          output: output.clone(),
          error,
        });
        state.checkpoint()?;
        if executed {
          options
            .hooks
            .post_tool(
              &self.workspace,
              session.id,
              &call,
              &output,
              error,
              &self.events,
            )
            .await;
        }
      }
      state.checkpoint()?;
      let steering = inbox
        .as_deref_mut()
        .map(|inbox| inbox.drain(final_reply))
        .unwrap_or_default();
      if steering.is_empty() && final_reply {
        return Ok(final_text);
      }
      for message in steering {
        self.events.emit(Event::Steering {
          message: message.clone(),
        });
        session.append(Message::text(Role::User, message));
      }
    }
    bail!("agent exceeded the {} step limit", options.max_steps)
  }
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
