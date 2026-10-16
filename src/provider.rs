use std::{
  collections::{BTreeMap, hash_map::RandomState},
  hash::{BuildHasher, Hasher},
  path::PathBuf,
  process::Stdio,
  time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, Response, StatusCode, header::RETRY_AFTER};
use serde::Deserialize;
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  process::Command,
};

use crate::{
  config::{PermissionMode, ProcessOutput},
  event::{Event, EventSink},
  process::GroupGuard,
  protocol::{Message, Role, ToolSpec, Usage},
  sse::SseDecoder,
};

const MAX_TRANSFER: usize = 16 * 1024 * 1024;

mod stream;
mod wire;

use stream::{Completion, StreamState};
use wire::{
  ChatRequest, PartialCall, StreamOptions, WireTool, parse_data, parse_response, wire_error,
  wire_message,
};

#[derive(Clone, Debug)]
pub struct ProviderReply {
  pub message: Message,
  pub usage: Usage,
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply>;
}

#[derive(Clone)]
pub enum RuntimeProvider {
  Http(HttpProvider),
  Process(ProcessProvider),
}

#[async_trait]
impl ChatProvider for RuntimeProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    match self {
      Self::Http(provider) => provider.complete(messages, tools, events).await,
      Self::Process(provider) => provider.complete(messages, tools, events).await,
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
    })
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
    let mut content = String::new();
    let mut calls: BTreeMap<usize, PartialCall> = BTreeMap::new();
    let mut usage = Usage::default();
    let mut finished = false;
    let mut done = false;
    let mut received = 0;
    'stream: while let Some(chunk) = stream.next().await {
      let chunk = chunk.context("read model stream")?;
      received += chunk.len();
      if received > MAX_TRANSFER {
        bail!("model stream exceeds the transfer limit");
      }
      for data in decoder.push(&chunk) {
        finished |= parse_data(&data, &mut content, &mut calls, &mut usage, events)?;
        if data.trim() == "[DONE]" {
          done = true;
          break 'stream;
        }
      }
    }
    if !done {
      for data in decoder.finish() {
        finished |= parse_data(&data, &mut content, &mut calls, &mut usage, events)?;
      }
    }

    if !finished {
      bail!("model stream ended before completion");
    }
    let tool_calls = calls
      .into_values()
      .map(PartialCall::finish)
      .collect::<Result<_>>()?;
    Ok(ProviderReply {
      message: Message {
        role: Role::Assistant,
        content: (!content.is_empty()).then_some(content),
        tool_calls,
        tool_call_id: None,
        images: Vec::new(),
      },
      usage,
    })
  }
}

#[derive(Clone)]
pub struct ProcessProvider {
  command: String,
  args: Vec<String>,
  model: String,
  workspace: PathBuf,
  permissions: PermissionMode,
  output: ProcessOutput,
}

impl ProcessProvider {
  pub fn new(
    command: String,
    args: Vec<String>,
    model: String,
    workspace: PathBuf,
    permissions: PermissionMode,
    output: ProcessOutput,
  ) -> Self {
    Self {
      command,
      args,
      model,
      workspace,
      permissions,
      output,
    }
  }

  fn args(&self) -> Vec<String> {
    let workspace = self.workspace.to_string_lossy();
    let sandbox = match self.permissions {
      PermissionMode::Auto => "workspace-write",
      PermissionMode::Ask | PermissionMode::ReadOnly => "read-only",
    };
    let permission = match self.permissions {
      PermissionMode::Auto => "acceptEdits",
      PermissionMode::Ask | PermissionMode::ReadOnly => "plan",
    };
    self
      .args
      .iter()
      .map(|arg| {
        arg
          .replace("{model}", &self.model)
          .replace("{workspace}", &workspace)
          .replace("{sandbox}", sandbox)
          .replace("{permission}", permission)
      })
      .collect()
  }
}

#[async_trait]
impl ChatProvider for ProcessProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    _tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    // built first so a turn the command cannot take fails before anything is started
    let prompt = render_prompt(messages, &self.command)?;
    let mut child = Command::new(&self.command)
      .args(self.args())
      .current_dir(&self.workspace)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .process_group(0)
      .spawn()
      .with_context(|| format!("start provider command {}", self.command))?;
    let _guard = GroupGuard::new(child.id());
    let mut stdin = child
      .stdin
      .take()
      .context("provider command stdin was unavailable")?;
    let stdout = child
      .stdout
      .take()
      .context("provider command stdout was unavailable")?;
    let stderr = child
      .stderr
      .take()
      .context("provider command stderr was unavailable")?;
    // the prompt is fed while output drains, so a chatty command cannot fill its pipe and stall.
    // a write error is left to the exit status: a command that quit early explains itself on stderr
    let feed = async move {
      drop(stdin.write_all(prompt.as_bytes()).await);
      drop(stdin.shutdown().await);
    };
    // stderr drains on its own task for the same reason the prompt is fed on one
    let errors = tokio::spawn(async move { crate::output::capture(stderr, MAX_TRANSFER).await });
    let mut stream = StreamState::default();
    let read = async {
      let mut reader = BufReader::new(stdout);
      let mut buffered = String::new();
      if self.output == ProcessOutput::StreamJson {
        let mut received = 0;
        loop {
          let mut line = String::new();
          let read = (&mut reader)
            .take((MAX_TRANSFER - received + 1) as u64)
            .read_line(&mut line)
            .await
            .context("read provider output")?;
          if read == 0 {
            break;
          }
          received += read;
          if received > MAX_TRANSFER {
            bail!("provider output exceeds the transfer limit");
          }
          stream.push(&line, events);
        }
      } else {
        // the other modes want the whole of stdout, so read it in one piece
        let captured = crate::output::capture(reader, MAX_TRANSFER)
          .await
          .context("read provider output")?;
        if captured.truncated {
          bail!("provider output exceeds the transfer limit");
        }
        buffered = String::from_utf8(captured.bytes).context("provider output was not UTF-8")?;
      }
      anyhow::Ok(buffered)
    };
    let (_, buffered) = tokio::join!(feed, read);
    let buffered = buffered?;
    let status = child.wait().await.context("wait for provider command")?;
    let errors = errors.await.context("read provider stderr")??;
    let errors = String::from_utf8_lossy(&errors.bytes);
    if !status.success() {
      // a command that dies early may explain itself on either pipe
      let detail = match errors.trim() {
        "" => buffered.trim(),
        error => error,
      };
      bail!("provider command failed ({status}): {detail}");
    }

    let reply = match self.output {
      ProcessOutput::Text => Completion::whole(buffered.trim().to_string()),
      ProcessOutput::JsonResult => Completion::whole(
        serde_json::from_str::<serde_json::Value>(buffered.trim())
          .context("invalid provider JSON output")?
          .get("result")
          .and_then(serde_json::Value::as_str)
          .context("provider JSON output had no result string")?
          .to_string(),
      ),
      ProcessOutput::StreamJson => stream.finish()?,
    };
    // text that already reached the screen as it arrived must not be sent a second time
    if !reply.text.is_empty() && !reply.streamed {
      events.emit(Event::TextDelta {
        text: reply.text.clone(),
      });
    }
    Ok(ProviderReply {
      message: Message::text(Role::Assistant, reply.text),
      usage: reply.usage,
    })
  }
}

// an image only survives if it is something the command can fetch for itself: inline data has
// nowhere to go in a text prompt, and dropping it silently would answer a question about a
// picture nobody sent
fn render_prompt(messages: &[&Message], command: &str) -> Result<String> {
  let mut prompt = String::new();
  for message in messages {
    let role = match message.role {
      Role::System => "system",
      Role::User => "user",
      Role::Assistant => "assistant",
      Role::Tool => "tool",
    };
    prompt.push_str(role);
    prompt.push_str(":\n");
    if let Some(content) = &message.content {
      prompt.push_str(content);
      prompt.push('\n');
    }
    for call in &message.tool_calls {
      prompt.push_str(&format!("tool call {}: {}\n", call.name, call.arguments));
    }
    for image in &message.images {
      if image.url.starts_with("data:") {
        bail!(
          "{command} takes a text prompt, so the image in this turn cannot be sent. Ask about \
           it with a provider that accepts images, or give the command the file path instead."
        );
      }
      prompt.push_str("[image: ");
      prompt.push_str(&image.url);
      prompt.push_str("]\n");
    }
    prompt.push('\n');
  }
  Ok(prompt)
}

#[cfg(test)]
#[path = "../tests/provider/retry.rs"]
mod retrying;
