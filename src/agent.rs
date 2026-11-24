use std::{
  future::Future,
  panic::AssertUnwindSafe,
  path::PathBuf,
  pin::Pin,
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::FutureExt;
use serde_json::Value;

use crate::{
  context::{message_bytes, tool_bytes},
  control::RunInbox,
  event::{Event, EventSink},
  protocol::{Image, Message, Role, ToolCall, ToolSpec, Usage},
  provider::{ChatProvider, ProviderReply},
  routing::{ModelTarget, RoutingStrategy},
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
use step::{Action, CONTINUATIONS, Effect, Turn, step};

pub use options::RunOptions;

pub struct RunSelection<P> {
  pub provider: Option<P>,
  pub target: ModelTarget,
  pub decision: Option<(RoutingStrategy, String)>,
  pub usage: Usage,
}

#[async_trait]
pub trait ModelSelector<P>: Send + Sync {
  async fn select(
    &self,
    session: &Session,
    prompt: &Message,
    continue_with: Option<&ModelTarget>,
  ) -> Result<RunSelection<P>>;
}

pub struct Agent<P> {
  provider: P,
  tools: ToolSet,
  workspace: PathBuf,
  events: EventSink,
  approver: Approver,
  selector: Option<Arc<dyn ModelSelector<P>>>,
  /// Bytes per token as the provider last counted them, stored as `f64` bits. It outlives a
  /// run, because the next run sends the same history to the same model.
  bytes_per_token: AtomicU64,
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
      selector: None,
      bytes_per_token: AtomicU64::new(context::BYTES_PER_TOKEN.to_bits()),
    }
  }

  pub fn provider(&self) -> &P {
    &self.provider
  }

  pub fn with_model_selector(mut self, selector: Arc<dyn ModelSelector<P>>) -> Self {
    self.selector = Some(selector);
    self
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
    let specs = self.tools.specs();
    // neither changes during a run, so neither is measured or rebuilt on every step
    let fixed = options.instructions.len() + tool_bytes(&specs);
    context::admit(&prompt, options, fixed, self.bytes_per_token())?;
    session.append(prompt);
    let failures = self.report_worker_failures(session);
    let system = Message::text(Role::System, &options.instructions);
    state.asks = Some(tool::asking(
      self.workspace.clone(),
      options,
      &self.approver,
    ));
    state.planning = self.tools.get(crate::todo::TOOL).is_some();
    if state.planning {
      state.plan = plan::recalled(session.context()?.messages);
    }
    let mut turn = Turn::new(options.max_steps);
    let mut final_text = failures.unwrap_or_default();
    let mut effect = step(&mut turn, Action::Begin);
    loop {
      state.checkpoint()?;
      if let (Some(limit), Some(cost)) = (options.budget_usd, state.usage.cost_usd)
        && cost >= limit
      {
        bail!(
          "run budget ${limit:.2} reached (reported cost ${cost:.2}); no further model requests or tools will run"
        );
      }
      effect = match effect {
        Effect::Ask | Effect::Continue => {
          let continuing = effect == Effect::Continue;
          if continuing {
            self.events.emit(Event::Notice {
              message: "the reply reached its output limit; continuing automatically".into(),
            });
          }
          let reply = self
            .ask(session, options, &system, &specs, fixed, continuing, state)
            .await?;
          final_text = reply.message.content.clone().unwrap_or_default();
          let calls = reply.message.tool_calls.len();
          state.pending = reply.message.tool_calls.iter().cloned().collect();
          session.append(reply.message);
          step(
            &mut turn,
            Action::Replied {
              calls,
              truncated: reply.truncated,
            },
          )
        }
        Effect::RunTools => {
          let progress = self.run_tools(session, options, state).await?;
          step(&mut turn, Action::ToolsDone { progress })
        }
        Effect::Drain => {
          let failure = self.report_worker_failures(session);
          let steering = inbox
            .as_deref_mut()
            .map(|inbox| inbox.drain(turn.settled && failure.is_none()))
            .unwrap_or_default();
          let messages = steering.len() + usize::from(failure.is_some());
          for message in steering {
            self.events.emit(Event::Steering {
              message: message.clone(),
            });
            session.append(Message::text(Role::User, message));
          }
          let next = step(&mut turn, Action::Steered { messages });
          if next == Effect::Pause
            && let Some(failure) = failure
          {
            final_text = failure;
          }
          next
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
        Effect::Stall => {
          self.events.emit(Event::Notice {
            message: format!(
              "paused after {CONTINUATIONS} automatic continuations hit the output limit \
               without a successful tool call; send a message to carry on"
            ),
          });
          return Ok(final_text);
        }
      };
    }
  }

  fn report_worker_failures(&self, session: &mut Session) -> Option<String> {
    let workers = self
      .tools
      .get("subagent")?
      .failed_background_workers(session.id);
    if workers.is_empty() {
      return None;
    }
    let notice = format!("Background subagents failed: {}.", workers.join(", "));
    session.append(Message::text(
      Role::User,
      format!(
        "[Ainz background worker status]\n{notice} Collect each failed worker for its error, \
         check what it already changed, and assign any remaining work needed for the current task."
      ),
    ));
    Some(notice)
  }

  #[allow(clippy::too_many_arguments)]
  async fn ask(
    &self,
    session: &mut Session,
    options: &RunOptions,
    system: &Message,
    specs: &[ToolSpec],
    fixed: usize,
    continuing: bool,
    state: &mut RunState,
  ) -> Result<ProviderReply> {
    // the instruction to carry on goes out with this one request and is not kept
    let continued;
    let (system, fixed) = if continuing {
      continued = Message::text(
        Role::System,
        format!("{}\n\n{}", options.instructions, context::CONTINUE),
      );
      (&continued, fixed + context::CONTINUE.len() + 2)
    } else {
      (system, fixed)
    };
    let asks = state.asks.clone().unwrap_or_else(deny_all);
    let mut tails = context::TAILS.into_iter();
    let mut resumed = 0;
    loop {
      let bytes = self.fit(session, options, fixed, &mut state.usage).await?;
      let context = session.context()?;
      let mut messages = Vec::with_capacity(context.len() + 1);
      messages.push(system);
      messages.extend(context.iter());
      state.checkpoint()?;
      // a provider that panics fails the run cleanly; the session it was handed is still whole
      let request =
        self
          .provider
          .complete_asking(&messages, specs, &self.events, options.permissions, &asks);
      let reply = AssertUnwindSafe(request)
        .catch_unwind()
        .await
        .map_err(|panic| anyhow!("the provider failed: {}", tool::panic_message(&*panic)))?;
      match reply {
        Ok(reply) => {
          accumulate(&mut state.usage, &reply.usage);
          self.measure(bytes, &reply.usage, options);
          return Ok(reply);
        }
        // the estimate came in low, which only the provider can say: believe it, and give up
        // more of the tail
        Err(error) if context::too_long(&error) => {
          self.learn(self.bytes_per_token() * 0.75);
          let mut archived = false;
          while !archived && let Some(preserve) = tails.next() {
            archived = self
              .compact(session, options, &mut state.usage, preserve)
              .await?;
          }
          if !archived {
            return Err(error);
          }
        }
        // a command that runs its own tools may have acted before it failed, so only a reply
        // that is nothing but words is asked for again
        Err(error)
          if resumed < context::RESUMES.len()
            && !self.provider.runs_tools()
            && context::interrupted(&error) =>
        {
          // what already streamed stays on screen, so say why it is about to repeat
          self.events.emit(Event::Notice {
            message: format!("the reply broke off ({error:#}); asking again"),
          });
          tokio::time::sleep(Duration::from_secs(context::RESUMES[resumed])).await;
          resumed += 1;
        }
        Err(error) => return Err(error),
      }
    }
  }

  /// Compacts when the history has grown past the threshold or would leave no room for the
  /// reply. When the preserved tail is itself what does not fit, compaction runs again keeping
  /// less of it, down to the latest prompt or the latest round of tool results. Returns the
  /// size of what will be sent, in bytes.
  async fn fit(
    &self,
    session: &mut Session,
    options: &RunOptions,
    fixed: usize,
    usage: &mut Usage,
  ) -> Result<usize> {
    let limit = context::room(options);
    let per_token = self.bytes_per_token();
    let mut bytes = sent(fixed, &session.context()?);
    let mut estimate = context::tokens(bytes, per_token);
    if (estimate >= options.compact_at_tokens || estimate > limit)
      && self
        .compact(session, options, usage, options.preserve_messages)
        .await?
    {
      bytes = sent(fixed, &session.context()?);
      estimate = context::tokens(bytes, per_token);
    }
    for preserve in context::TAILS {
      if estimate > limit && self.compact(session, options, usage, preserve).await? {
        bytes = sent(fixed, &session.context()?);
        estimate = context::tokens(bytes, per_token);
      }
    }
    if estimate > limit {
      bail!(
        "the latest messages are about {estimate} tokens even after compacting, and only \
         {limit} fit in a {} token context with room left for a reply; send another prompt to \
         carry on without them, or raise context_tokens if this model takes more",
        options.context_tokens
      );
    }
    Ok(bytes)
  }

  fn bytes_per_token(&self) -> f64 {
    f64::from_bits(self.bytes_per_token.load(Ordering::Relaxed))
  }

  /// Clamped, because one odd reply (a provider that counts only the fresh part of a cached
  /// prompt, say) must not teach the estimate something absurd.
  fn learn(&self, per_token: f64) {
    self
      .bytes_per_token
      .store(per_token.clamp(1.5, 4.5).to_bits(), Ordering::Relaxed);
  }

  /// What the provider counted against what was sent, so the next estimate is the provider's
  /// rather than a guess, and the context left is what the provider says it is. A command that
  /// runs its own loop counts its own context, which says nothing about this one.
  fn measure(&self, bytes: usize, usage: &Usage, options: &RunOptions) {
    let counted = match self.provider.runs_tools() {
      true => 0,
      false => usage.input_tokens as usize,
    };
    if counted > 0 && bytes > 0 {
      self.learn(bytes as f64 / counted as f64);
    }
    let used = match counted {
      0 => context::tokens(bytes, self.bytes_per_token()),
      counted => counted,
    };
    self.events.emit(Event::Context {
      used,
      limit: options.context_tokens,
    });
  }
}

fn sent(fixed: usize, context: &Context<'_>) -> usize {
  fixed + context.iter().map(message_bytes).sum::<usize>()
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
