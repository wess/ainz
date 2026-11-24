use std::{
  collections::{BTreeMap, hash_map::RandomState},
  hash::{BuildHasher, Hasher},
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, Response, StatusCode, header::RETRY_AFTER};
use serde::Deserialize;

use crate::{
  agent::Approver,
  config::PermissionMode,
  event::{Event, EventSink},
  protocol::{Message, ToolSpec, Usage},
  routing::ModelTarget,
  sse::SseDecoder,
};

const MAX_TRANSFER: usize = 16 * 1024 * 1024;

/// What a chat-completions reply may run to unless a profile says otherwise. A request that
/// names no limit gets the server's, and a LiteLLM proxy in front of Claude picks 4,096.
pub const OUTPUT_LIMIT: usize = 32_000;

mod anthropic;
mod bridge;
mod input;
mod process;
mod responses;
mod stream;
mod transport;
pub(crate) mod wire;
mod workers;
pub use anthropic::AnthropicProvider;
pub use process::ProcessProvider;
pub use responses::ResponsesProvider;
use transport::{Wire, default_header, read_events};
use wire::error_message;
fn excerpt(data: &str) -> String {
  data.chars().take(200).collect()
}
fn arguments(raw: &str) -> serde_json::Value {
  serde_json::from_str::<serde_json::Value>(raw)
    .ok()
    .filter(serde_json::Value::is_object)
    .unwrap_or_else(|| {
      if raw.trim().is_empty() {
        serde_json::json!({})
      } else {
        serde_json::json!({ crate::protocol::INVALID_ARGUMENTS: raw })
      }
    })
}

use wire::{
  ChatRequest, ChatStream, StreamOptions, WireTool, parse_response, wire_error, wire_message,
};

#[derive(Clone, Debug)]
pub struct ProviderReply {
  pub message: Message,
  pub usage: Usage,
  /// The reply stopped at the provider's output limit rather than finishing.
  pub truncated: bool,
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply>;

  /// The same request made from inside a run, which says what mode the run is in and who
  /// answers for a tool the provider runs itself. Only a command running its own loop has
  /// tools of its own; a provider that hands its calls back to the agent has nothing to ask.
  async fn complete_asking(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
    _mode: PermissionMode,
    _asks: &Approver,
  ) -> Result<ProviderReply> {
    self.complete(messages, tools, events).await
  }

  /// Whether the provider runs tools of its own, as a headless coding command does. Such a
  /// reply that broke off may already have acted, so it is never simply asked for again, and
  /// its token counts describe the command's context rather than the one sent to it.
  fn runs_tools(&self) -> bool {
    false
  }
}

#[derive(Clone)]
pub enum RuntimeProvider {
  Http(HttpProvider),
  Anthropic(AnthropicProvider),
  Responses(ResponsesProvider),
  Fallback(Vec<(String, Box<RuntimeProvider>)>),
  Process(ProcessProvider),
}

impl RuntimeProvider {
  pub fn with_model(&self, model: String) -> Self {
    match self {
      // another model may take the limit this one refused
      Self::Http(provider) => Self::Http(HttpProvider {
        model,
        limited: Arc::new(AtomicBool::new(true)),
        ..provider.clone()
      }),
      Self::Process(provider) => Self::Process(provider.with_model(model)),
      Self::Anthropic(provider) => Self::Anthropic(provider.with_model(model)),
      Self::Responses(provider) => Self::Responses(provider.with_model(model)),
      Self::Fallback(providers) => {
        let mut providers = providers.clone();
        // a model belongs to its provider; the backups keep the ones configured for them
        if let Some((_, primary)) = providers.first_mut() {
          **primary = primary.with_model(model);
        }
        Self::Fallback(providers)
      }
    }
  }

  pub async fn from_profile(
    config: &crate::Config,
    profile: &crate::ProviderConfig,
    model: String,
    workspace: &std::path::Path,
  ) -> Result<Self> {
    use crate::config::{ProviderKind, WireApi};
    profile.validate("selected")?;
    if profile.kind == ProviderKind::Process {
      return Ok(Self::Process(ProcessProvider::new(
        profile
          .command
          .clone()
          .context("process provider requires a command")?,
        profile.args.clone(),
        model,
        workspace.to_path_buf(),
        config.permissions,
        profile.output,
      )));
    }
    let endpoint = profile
      .endpoint
      .clone()
      .context("HTTP provider requires an endpoint")?;
    let key = config.api_key_for(profile).await?;
    Ok(match profile.wire_api {
      WireApi::Chat => {
        let mut provider = HttpProvider::new(endpoint, model, key, config.provider_retries)?
          .with_output_limit(profile.max_output_tokens);
        provider.headers = profile.headers.clone();
        Self::Http(provider)
      }
      WireApi::Messages => Self::Anthropic(AnthropicProvider::new(
        endpoint,
        model,
        key,
        profile.headers.clone(),
        profile.thinking_tokens,
        config.provider_retries,
      )?),
      WireApi::Responses => Self::Responses(ResponsesProvider::new(
        endpoint,
        model,
        key,
        profile.headers.clone(),
        config.provider_retries,
        profile.reasoning_effort.clone(),
      )?),
    })
  }

  pub async fn configured(config: &crate::Config, workspace: &std::path::Path) -> Result<Self> {
    Self::configured_for(config, &ModelTarget::configured(config), workspace).await
  }

  pub async fn from_target(
    config: &crate::Config,
    target: &ModelTarget,
    workspace: &std::path::Path,
  ) -> Result<Self> {
    let provider = Self::from_profile(
      config,
      &target.profile(config)?,
      target.model.clone(),
      workspace,
    )
    .await?;
    Ok(match provider {
      Self::Anthropic(provider) => {
        Self::Anthropic(provider.with_profile(target.provider.as_deref()))
      }
      Self::Responses(provider) => {
        Self::Responses(provider.with_profile(target.provider.as_deref()))
      }
      provider => provider,
    })
  }

  pub async fn configured_for(
    config: &crate::Config,
    target: &ModelTarget,
    workspace: &std::path::Path,
  ) -> Result<Self> {
    let active = Self::from_target(config, target, workspace).await?;
    if config.fallback_providers.is_empty() {
      return Ok(active);
    }
    let mut providers = vec![(target.provider_name().to_string(), Box::new(active))];
    for name in &config.fallback_providers {
      if target.provider.as_ref() == Some(name) {
        continue;
      }
      let profile = config
        .providers
        .get(name)
        .with_context(|| format!("fallback provider {name} is not configured"))?;
      let model = profile
        .model
        .clone()
        .unwrap_or_else(|| target.model.clone());
      let fallback = ModelTarget {
        provider: Some(name.clone()),
        model,
      };
      providers.push((
        name.clone(),
        Box::new(Self::from_target(config, &fallback, workspace).await?),
      ));
    }
    Ok(Self::Fallback(providers))
  }

  pub async fn models(&self) -> Result<Vec<String>> {
    match self {
      Self::Http(provider) => provider.models().await,
      Self::Anthropic(provider) => provider.models().await,
      Self::Responses(provider) => provider.models().await,
      _ => bail!("model discovery requires a single HTTP provider"),
    }
  }
}

#[async_trait]
impl ChatProvider for RuntimeProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    self.reply(messages, tools, events, None).await
  }

  async fn complete_asking(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
    mode: PermissionMode,
    asks: &Approver,
  ) -> Result<ProviderReply> {
    self
      .reply(messages, tools, events, Some((mode, asks)))
      .await
  }

  fn runs_tools(&self) -> bool {
    match self {
      Self::Process(_) => true,
      Self::Fallback(providers) => providers.iter().any(|(_, provider)| provider.runs_tools()),
      _ => false,
    }
  }
}

impl RuntimeProvider {
  async fn reply(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
    asking: Option<(PermissionMode, &Approver)>,
  ) -> Result<ProviderReply> {
    match self {
      Self::Http(provider) => provider.complete(messages, tools, events).await,
      Self::Anthropic(provider) => provider.complete(messages, tools, events).await,
      Self::Responses(provider) => provider.complete(messages, tools, events).await,
      Self::Process(provider) => match asking {
        Some((mode, asks)) => {
          provider
            .complete_asking(messages, tools, events, mode, asks)
            .await
        }
        None => provider.complete(messages, tools, events).await,
      },
      Self::Fallback(providers) => {
        let mut errors = Vec::new();
        for (index, (name, provider)) in providers.iter().enumerate() {
          let (buffer, mut received) = EventSink::channel();
          let attempt = match asking {
            Some((mode, asks)) => {
              Box::pin(provider.complete_asking(messages, tools, &buffer, mode, asks)).await
            }
            None => Box::pin(provider.complete(messages, tools, &buffer)).await,
          };
          match attempt {
            Ok(reply) => {
              if index > 0 {
                events.emit(Event::Notice {
                  message: format!("provider fallback selected {name}"),
                });
              }
              while let Ok(event) = received.try_recv() {
                events.emit(event);
              }
              return Ok(reply);
            }
            Err(error) => errors.push(format!("{name}: {error:#}")),
          }
        }
        bail!("all configured providers failed: {}", errors.join("; "))
      }
    }
  }
}

// retry only for transient conditions: an overloaded or momentarily unreachable upstream,
// never a client error that sending the same request again cannot fix
fn retryable_status(status: StatusCode) -> bool {
  status == StatusCode::REQUEST_TIMEOUT
    || status == StatusCode::TOO_MANY_REQUESTS
    || status.is_server_error()
}

// a body that names one of these is refused for who is asking, not for how busy the server is,
// so asking again only makes the person wait longer for the same sentence
fn permanent_error(body: &str) -> bool {
  let body = body.to_ascii_lowercase();
  [
    "access_denied",
    "permission_denied",
    "not entitled",
    "insufficient_quota",
    "invalid_api_key",
  ]
  .iter()
  .any(|cause| body.contains(cause))
}

// Retry-After is sometimes an HTTP-date rather than a second count; that form is left alone
// and the exponential backoff below is used instead of trying to parse it. a server asking for
// longer than a minute gets a minute: a turn that silently sleeps for an hour looks hung
fn retry_after(response: &Response) -> Option<Duration> {
  let seconds = response
    .headers()
    .get(RETRY_AFTER)?
    .to_str()
    .ok()?
    .trim()
    .parse::<u64>()
    .ok()?;
  Some(Duration::from_secs(seconds.min(60)))
}

// 500ms, 1s, 2s, 4s, capped at 8s, jittered by roughly a quarter so a pile of retrying clients
// doesn't all wake up on the same tick; a fresh hasher's randomized keys are enough entropy for
// that without pulling in a rand dependency
fn backoff(attempt: usize) -> Duration {
  let exponent = attempt.saturating_sub(1).min(4) as u32;
  let base_ms = 500u64 << exponent;
  let wobble = base_ms / 4;
  let sample = RandomState::new().build_hasher().finish() % (wobble * 2 + 1);
  Duration::from_millis(base_ms - wobble + sample)
}

fn format_wait(wait: Duration) -> String {
  let millis = wait.as_millis();
  if millis < 1000 {
    format!("{millis}ms")
  } else if millis.is_multiple_of(1000) {
    format!("{}s", millis / 1000)
  } else {
    format!("{:.1}s", millis as f64 / 1000.0)
  }
}

// sends a request, and sends it again while the reason it failed might be gone a moment later.
// `build` is a closure rather than a cloned builder because a body is consumed by the attempt
// that used it. retrying stops at the response: once a stream under a success status starts
// handing text to the caller, a retry would replay output somebody has already read
async fn send(
  what: &str,
  retries: usize,
  events: Option<&EventSink>,
  build: impl Fn() -> RequestBuilder,
) -> Result<Response> {
  let attempts = retries + 1;
  let mut attempt = 0usize;
  loop {
    attempt += 1;
    let tries = match attempt {
      1 => String::new(),
      _ => format!(" after {attempt} attempts"),
    };
    let (wait, reason) = match build().send().await {
      // a request that could not be built is the same request next time
      Err(error) if error.is_builder() || attempt == attempts => {
        return Err(error).with_context(|| format!("{what} failed{tries}"));
      }
      Err(error) => (backoff(attempt), error.to_string()),
      Ok(response) if response.status().is_success() => return Ok(response),
      Ok(response) => {
        let status = response.status();
        let wait = retry_after(&response).unwrap_or_else(|| backoff(attempt));
        let body = crate::output::response(response, 65_536)
          .await
          .unwrap_or_default();
        let body = String::from_utf8_lossy(&body);
        let error = wire_error(status, &body);
        if !retryable_status(status) || permanent_error(&body) || attempt == attempts {
          bail!("{what} failed{tries}: {error}");
        }
        (wait, error)
      }
    };
    if let Some(events) = events {
      events.emit(Event::Error {
        message: format!(
          "{what} failed ({reason}); retrying in {} (attempt {} of {attempts})",
          format_wait(wait),
          attempt + 1,
        ),
      });
    }
    tokio::time::sleep(wait).await;
  }
}

#[derive(Clone)]
pub struct HttpProvider {
  client: Client,
  endpoint: String,
  model: String,
  api_key: Option<String>,
  retries: usize,
  headers: BTreeMap<String, String>,
  /// The most a reply may run to, in tokens; zero sends no limit.
  output_limit: usize,
  /// Cleared once the server refuses the limit, so it is asked without one from then on
  /// rather than refused again on every request.
  limited: Arc<AtomicBool>,
}

impl HttpProvider {
  pub fn new(
    endpoint: String,
    model: String,
    api_key: Option<String>,
    retries: usize,
  ) -> Result<Self> {
    // no overall timeout, because a local model can sit for minutes before its first token; a
    // stream silent for five, though, is a connection that died under it (a laptop that slept,
    // a gateway that dropped it) and would otherwise hang the turn for ever
    let client = Client::builder()
      .connect_timeout(Duration::from_secs(15))
      .read_timeout(Duration::from_secs(300))
      .build()
      .context("build HTTP client")?;
    Ok(Self {
      client,
      endpoint: endpoint.trim_end_matches('/').into(),
      model,
      api_key,
      retries,
      headers: BTreeMap::new(),
      output_limit: OUTPUT_LIMIT,
      limited: Arc::new(AtomicBool::new(true)),
    })
  }

  /// A profile's own output limit, or the default when it names none.
  pub fn with_output_limit(mut self, tokens: Option<usize>) -> Self {
    self.output_limit = tokens.unwrap_or(OUTPUT_LIMIT);
    self
  }

  pub async fn models(&self) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct ModelList {
      data: Vec<Model>,
    }

    #[derive(Deserialize)]
    struct Model {
      id: String,
    }

    // discovery runs during setup, before an agent (and its event sink) exists, so a retry
    // here has nothing to report through — only the eventual outcome is visible to the caller
    let url = format!("{}/models", self.endpoint);
    let response = send("model discovery", self.retries, None, || {
      self.authorize(self.client.get(&url))
    })
    .await?;
    let mut models: Vec<_> =
      serde_json::from_slice::<ModelList>(&crate::output::response(response, MAX_TRANSFER).await?)
        .context("invalid model list")?
        .data
        .into_iter()
        .map(|model| model.id)
        .collect();
    models.sort();
    models.dedup();
    Ok(models)
  }

  fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
    let request = self.headers.iter().fold(request, |request, (name, value)| {
      request.header(name, value)
    });
    match &self.api_key {
      Some(key) => request.bearer_auth(key),
      None => request,
    }
  }
}

#[async_trait]
impl ChatProvider for HttpProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    let limit =
      (self.output_limit > 0 && self.limited.load(Ordering::Relaxed)).then_some(self.output_limit);
    match self.request(messages, tools, events, limit).await {
      Err(error) if limit.is_some() && refused_limit(&error) => {
        self.limited.store(false, Ordering::Relaxed);
        events.emit(Event::Notice {
          message: format!(
            "the provider refused an output limit of {} tokens; asking without one",
            self.output_limit
          ),
        });
        self.request(messages, tools, events, None).await
      }
      reply => reply,
    }
  }
}

/// Whether a request was refused for the output limit it named, as a model with a smaller
/// ceiling refuses one it cannot honour. A refusal for the whole request being too long also
/// names the limit, and asking without one would not make that request any shorter.
fn refused_limit(error: &anyhow::Error) -> bool {
  let text = format!("{error:#}").to_ascii_lowercase();
  (text.contains("max_completion_tokens") || text.contains("max_tokens"))
    && !text.contains("context")
}

impl HttpProvider {
  async fn request(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
    limit: Option<usize>,
  ) -> Result<ProviderReply> {
    let request = ChatRequest {
      model: &self.model,
      messages: messages
        .iter()
        .map(|message| wire_message(message))
        .collect(),
      stream: true,
      stream_options: StreamOptions {
        include_usage: true,
      },
      tools: tools.iter().map(WireTool::new).collect(),
      tool_choice: (!tools.is_empty()).then_some("auto"),
      max_completion_tokens: limit,
    };
    // serialized once; a retry sends the same bytes again rather than encoding them anew
    let body = bytes::Bytes::from(serde_json::to_vec(&request)?);
    drop(request);
    let url = format!("{}/chat/completions", self.endpoint);
    let response = send("model request", self.retries, Some(events), || {
      self
        .authorize(self.client.post(&url))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.clone())
    })
    .await?;

    let streaming = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .is_some_and(|value| value.contains("text/event-stream"));
    if !streaming {
      return parse_response(
        &crate::output::response(response, MAX_TRANSFER).await?,
        events,
      );
    }

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut reply = ChatStream::default();
    let mut done = false;
    let mut received = 0;
    'stream: while let Some(chunk) = stream.next().await {
      let chunk = chunk.context("read model stream")?;
      received += chunk.len();
      if received > MAX_TRANSFER {
        bail!("model stream exceeds the transfer limit");
      }
      for data in decoder.push(&chunk) {
        if reply.push(&data, events)? {
          done = true;
          break 'stream;
        }
      }
    }
    if !done {
      for data in decoder.finish() {
        reply.push(&data, events)?;
      }
    }
    reply.finish()
  }
}

#[cfg(test)]
#[path = "../tests/provider/retry.rs"]
mod retrying;
