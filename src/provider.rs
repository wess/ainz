use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{
  config::{PermissionMode, ProcessOutput},
  event::{Event, EventSink},
  protocol::{Message, Role, ToolSpec, Usage},
  sse::SseDecoder,
};

mod wire;

use wire::{PartialCall, parse_data, parse_response, wire_message};

#[derive(Clone, Debug)]
pub struct ProviderReply {
  pub message: Message,
  pub usage: Usage,
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
  async fn complete(
    &self,
    messages: &[Message],
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
    messages: &[Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    match self {
      Self::Http(provider) => provider.complete(messages, tools, events).await,
      Self::Process(provider) => provider.complete(messages, tools, events).await,
    }
  }
}

#[derive(Clone)]
pub struct HttpProvider {
  client: Client,
  endpoint: String,
  model: String,
  api_key: Option<String>,
}

impl HttpProvider {
  pub fn new(endpoint: String, model: String, api_key: Option<String>) -> Result<Self> {
    // NB: no read timeout on purpose; a local model can sit for minutes before its first token
    let client = Client::builder()
      .connect_timeout(Duration::from_secs(15))
      .build()
      .context("build HTTP client")?;
    Ok(Self {
      client,
      endpoint: endpoint.trim_end_matches('/').into(),
      model,
      api_key,
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

    let mut request = self.client.get(format!("{}/models", self.endpoint));
    if let Some(key) = &self.api_key {
      request = request.bearer_auth(key);
    }
    let response = request.send().await.context("list provider models")?;
    let status = response.status();
    if !status.is_success() {
      bail!(
        "model discovery failed ({status}): {}",
        response.text().await.unwrap_or_default()
      );
    }
    let mut models: Vec<_> = response
      .json::<ModelList>()
      .await
      .context("invalid model list")?
      .data
      .into_iter()
      .map(|model| model.id)
      .collect();
    models.sort();
    models.dedup();
    Ok(models)
  }
}

#[async_trait]
impl ChatProvider for HttpProvider {
  async fn complete(
    &self,
    messages: &[Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    let mut body = json!({
      "model": self.model,
      "messages": messages.iter().map(wire_message).collect::<Vec<_>>(),
      "stream": true,
      "stream_options": {"include_usage": true},
    });
    // an empty tools array with tool_choice is rejected by several OpenAI-compatible servers
    if !tools.is_empty() {
      body["tools"] = tools
        .iter()
        .map(|tool| json!({"type": "function", "function": tool}))
        .collect();
      body["tool_choice"] = json!("auto");
    }
    let mut request = self
      .client
      .post(format!("{}/chat/completions", self.endpoint))
      .json(&body);
    if let Some(key) = &self.api_key {
      request = request.bearer_auth(key);
    }
    let response = request.send().await.context("send model request")?;
    let status = response.status();
    if !status.is_success() {
      bail!(
        "model request failed ({status}): {}",
        response.text().await.unwrap_or_default()
      );
    }

    let streaming = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .is_some_and(|value| value.contains("text/event-stream"));
    if !streaming {
      return parse_response(
        response.json().await.context("invalid model response")?,
        events,
      );
    }

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut content = String::new();
    let mut calls: BTreeMap<usize, PartialCall> = BTreeMap::new();
    let mut usage = Usage::default();
    while let Some(chunk) = stream.next().await {
      for data in decoder.push(&chunk.context("read model stream")?) {
        parse_data(&data, &mut content, &mut calls, &mut usage, events)?;
      }
    }
    for data in decoder.finish() {
      parse_data(&data, &mut content, &mut calls, &mut usage, events)?;
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
    messages: &[Message],
    _tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    let mut child = Command::new(&self.command)
      .args(self.args())
      .current_dir(&self.workspace)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .spawn()
      .with_context(|| format!("start provider command {}", self.command))?;
    let mut stdin = child
      .stdin
      .take()
      .context("provider command stdin was unavailable")?;
    let prompt = render_prompt(messages);
    // the prompt is fed while output drains, so a chatty command cannot fill its pipe and stall.
    // a write error is left to the exit status: a command that quit early explains itself on stderr
    let feed = async move {
      drop(stdin.write_all(prompt.as_bytes()).await);
      drop(stdin.shutdown().await);
    };
    let (_, output) = tokio::join!(feed, child.wait_with_output());
    let output = output.context("wait for provider command")?;
    if !output.status.success() {
      let error = String::from_utf8_lossy(&output.stderr);
      bail!(
        "provider command failed ({}): {}",
        output.status,
        error.trim()
      );
    }
    let stdout = String::from_utf8(output.stdout).context("provider output was not UTF-8")?;
    let text = match self.output {
      ProcessOutput::Text => stdout.trim().to_string(),
      ProcessOutput::JsonResult => serde_json::from_str::<serde_json::Value>(&stdout)
        .context("invalid provider JSON output")?
        .get("result")
        .and_then(serde_json::Value::as_str)
        .context("provider JSON output had no result string")?
        .to_string(),
    };
    if !text.is_empty() {
      events.emit(Event::TextDelta { text: text.clone() });
    }
    Ok(ProviderReply {
      message: Message::text(Role::Assistant, text),
      usage: Usage::default(),
    })
  }
}

fn render_prompt(messages: &[Message]) -> String {
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
    if !message.images.is_empty() {
      prompt.push_str("[image content omitted by process provider]\n");
    }
    prompt.push('\n');
  }
  prompt
}
