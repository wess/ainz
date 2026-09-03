use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::{
  config::{PermissionMode, PermissionRules},
  context::{estimate_tokens, transcript},
  control::{RunInbox, RunSignal},
  event::{Event, EventSink},
  protocol::{Image, Message, Role, ToolCall, Usage},
  provider::ChatProvider,
  session::Session,
  tool::{Risk, ToolContext, ToolSet},
};

pub type Approval = Pin<Box<dyn Future<Output = bool> + Send>>;
pub type Approver = Arc<dyn Fn(&ToolCall, Risk) -> Approval + Send + Sync>;

// for surfaces with nobody to ask: json output, rpc, tests
pub fn deny_all() -> Approver {
  Arc::new(|_, _| Box::pin(async { false }))
}

#[derive(Clone)]
pub struct RunOptions {
  pub instructions: String,
  pub permissions: PermissionMode,
  // what may run without asking, whatever the mode
  pub rules: PermissionRules,
  pub max_steps: usize,
  pub max_output_bytes: usize,
  pub context_tokens: usize,
  pub compact_at_tokens: usize,
  pub preserve_messages: usize,
  // asked of the model once a compaction has archived messages, when memory is on
  pub memory_nudge: Option<String>,
}

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

  async fn run_message(
    &self,
    session: &mut Session,
    prompt: Message,
    options: RunOptions,
    mut inbox: Option<&mut RunInbox>,
  ) -> Result<String> {
    session.append(prompt);
    let mut total = Usage::default();
    let specs = self.tools.specs();
    let mut steering = Vec::new();
    for _ in 0..options.max_steps {
      let mut context_messages = session.context_messages()?;
      let mut estimate = estimate_tokens(&options.instructions, &context_messages, &specs);
      if estimate >= options.compact_at_tokens
        && self.compact(session, &options, &mut total).await?
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
      let completion = self.provider.complete(&messages, &specs, &self.events);
      tokio::pin!(completion);
      let reply = loop {
        if let Some(receiver) = inbox.as_deref_mut()
          && receiver.is_open()
        {
          tokio::select! {
            result = &mut completion => break result?,
            signal = receiver.receive() => {
              if self.handle_signal(signal, &mut steering) {
                record_usage(session, &total);
                self.events.emit(Event::Cancelled);
                bail!("run cancelled");
              }
            }
          }
        } else {
          break completion.await?;
        }
      };
      total.input_tokens += reply.usage.input_tokens;
      total.output_tokens += reply.usage.output_tokens;
      total.cost_usd = add_cost(total.cost_usd, reply.usage.cost_usd);
      let final_text = reply.message.content.clone().unwrap_or_default();
      let calls = reply.message.tool_calls.clone();
      session.append(reply.message);
      if self.drain_signals(&mut inbox, &mut steering) {
        record_usage(session, &total);
        self.events.emit(Event::Cancelled);
        bail!("run cancelled");
      }
      if calls.is_empty() && steering.is_empty() {
        record_usage(session, &total);
        self.events.emit(Event::TurnEnd { usage: total });
        return Ok(final_text);
      }

      for call in calls {
        self.events.emit(Event::ToolStart { call: call.clone() });
        let execution = self.run_tool(&call, &options, session.id);
        tokio::pin!(execution);
        let (output, error) = loop {
          if let Some(receiver) = inbox.as_deref_mut()
            && receiver.is_open()
          {
            tokio::select! {
              result = &mut execution => break result,
              signal = receiver.receive() => {
                if self.handle_signal(signal, &mut steering) {
                  record_usage(session, &total);
                  self.events.emit(Event::Cancelled);
                  bail!("run cancelled");
                }
              }
            }
          } else {
            break execution.await;
          }
        };
        self.events.emit(Event::ToolEnd {
          id: call.id.clone(),
          output: output.clone(),
          error,
        });
        session.append(Message::tool(call.id.clone(), output));
      }
      if self.drain_signals(&mut inbox, &mut steering) {
        record_usage(session, &total);
        self.events.emit(Event::Cancelled);
        bail!("run cancelled");
      }
      for message in steering.drain(..) {
        self.events.emit(Event::Steering {
          message: message.clone(),
        });
        session.append(Message::text(Role::User, message));
      }
    }
    record_usage(session, &total);
    bail!("agent exceeded the {} step limit", options.max_steps)
  }

  fn handle_signal(&self, signal: Option<RunSignal>, steering: &mut Vec<String>) -> bool {
    match signal {
      Some(RunSignal::Steer(message)) if !message.trim().is_empty() => {
        steering.push(message);
        false
      }
      Some(RunSignal::Cancel) => true,
      _ => false,
    }
  }

  fn drain_signals(&self, inbox: &mut Option<&mut RunInbox>, steering: &mut Vec<String>) -> bool {
    let Some(receiver) = inbox.as_deref_mut() else {
      return false;
    };
    while let Some(signal) = receiver.try_receive() {
      if self.handle_signal(Some(signal), steering) {
        return true;
      }
    }
    false
  }

  // returns whether anything was archived; nothing happens when too little is old enough
  async fn compact(
    &self,
    session: &mut Session,
    options: &RunOptions,
    total: &mut Usage,
  ) -> Result<bool> {
    let Some((cursor, input, archived_messages)) =
      session.compaction_input(options.preserve_messages)?
    else {
      return Ok(false);
    };
    let request = vec![
      Message::text(
        Role::System,
        concat!(
          "Summarize the session transcript for another agent continuing the work. ",
          "Preserve decisions, constraints, file paths, commands and results, unresolved ",
          "errors, and the next concrete action. Omit conversational filler."
        ),
      ),
      Message::text(Role::User, transcript(&input)),
    ];
    let reply = self
      .provider
      .complete(&request, &[], &EventSink::default())
      .await?;
    total.input_tokens += reply.usage.input_tokens;
    total.output_tokens += reply.usage.output_tokens;
    total.cost_usd = add_cost(total.cost_usd, reply.usage.cost_usd);
    let summary = reply.message.content.unwrap_or_default();
    if summary.trim().is_empty() {
      bail!("context compaction returned an empty summary");
    }
    session.record_summary(cursor, summary.clone())?;
    // the one moment where not having written something down costs immediately
    if let Some(nudge) = options.memory_nudge.as_deref() {
      session.append(Message::text(Role::System, nudge));
    }
    self.events.emit(Event::Compaction {
      archived_messages,
      summary,
    });
    Ok(true)
  }

  async fn run_tool(
    &self,
    call: &ToolCall,
    options: &RunOptions,
    session_id: uuid::Uuid,
  ) -> (String, bool) {
    let Some(tool) = self.tools.get(&call.name) else {
      return (format!("unknown tool: {}", call.name), true);
    };
    let risk = tool.risk(&call.arguments);
    // a standing rule answers before anyone is asked, in every mode: it is the same decision,
    // made once already
    let ruled = options.rules.decide(&call.name, subject(&call.arguments));
    let allowed = match (ruled, options.permissions) {
      (Some(decided), _) => decided,
      (None, PermissionMode::Auto) => true,
      (None, PermissionMode::ReadOnly) => risk == Risk::Read,
      (None, PermissionMode::Ask) => risk == Risk::Read || (self.approver)(call, risk).await,
    };
    if !allowed {
      return (
        format!("permission denied for {} ({risk:?})", call.name),
        true,
      );
    }
    let context = ToolContext {
      workspace: self.workspace.clone(),
      session_id,
      max_output_bytes: options.max_output_bytes,
      progress: Some((self.events.clone(), call.id.clone())),
    };
    match tool.execute(&context, call.arguments.clone()).await {
      Ok(output) => (output, false),
      Err(error) => (format!("{error:#}"), true),
    }
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

fn record_usage(session: &mut Session, usage: &Usage) {
  session.usage.input_tokens += usage.input_tokens;
  session.usage.output_tokens += usage.output_tokens;
  session.usage.cost_usd = add_cost(session.usage.cost_usd, usage.cost_usd);
}

// a cost nobody reported is not zero, it is unknown, so it stays None until one is
fn add_cost(total: Option<f64>, next: Option<f64>) -> Option<f64> {
  match (total, next) {
    (Some(total), Some(next)) => Some(total + next),
    (total, next) => total.or(next),
  }
}
