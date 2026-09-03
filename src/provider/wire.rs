use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ProviderReply;
use crate::{
  event::{Event, EventSink},
  protocol::{Message, Role, ToolCall, Usage},
};

pub(super) fn wire_message(message: &Message) -> Value {
  let role = match message.role {
    Role::System => "system",
    Role::User => "user",
    Role::Assistant => "assistant",
    Role::Tool => "tool",
  };
  let content = if message.images.is_empty() {
    json!(message.content)
  } else {
    let mut parts = Vec::new();
    if let Some(text) = &message.content {
      parts.push(json!({"type": "text", "text": text}));
    }
    parts.extend(message.images.iter().map(|image| {
      let mut source = json!({"url": image.url});
      if let Some(detail) = &image.detail {
        source["detail"] = json!(detail);
      }
      json!({"type": "image_url", "image_url": source})
    }));
    Value::Array(parts)
  };
  let mut value = json!({"role": role, "content": content});
  if !message.tool_calls.is_empty() {
    value["tool_calls"] = Value::Array(
      message
        .tool_calls
        .iter()
        .map(|call| {
          json!({
            "id": call.id, "type": "function", "function": {
              "name": call.name, "arguments": call.arguments.to_string()
            }
          })
        })
        .collect(),
    );
  }
  if let Some(id) = &message.tool_call_id {
    value["tool_call_id"] = Value::String(id.clone());
  }
  value
}

#[derive(Default)]
pub(super) struct PartialCall {
  id: String,
  name: String,
  arguments: String,
}

impl PartialCall {
  pub(super) fn finish(self) -> Result<ToolCall> {
    Ok(ToolCall {
      id: self.id,
      name: self.name,
      arguments: if self.arguments.is_empty() {
        json!({})
      } else {
        serde_json::from_str(&self.arguments)?
      },
    })
  }
}

#[derive(Deserialize)]
struct StreamChunk {
  #[serde(default)]
  choices: Vec<Choice>,
  usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct Choice {
  delta: Delta,
}

#[derive(Deserialize)]
struct Delta {
  content: Option<String>,
  #[serde(default)]
  tool_calls: Vec<CallDelta>,
}

#[derive(Deserialize)]
struct CallDelta {
  index: usize,
  id: Option<String>,
  function: Option<FunctionDelta>,
}

#[derive(Deserialize)]
struct FunctionDelta {
  name: Option<String>,
  arguments: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
  prompt_tokens: u64,
  completion_tokens: u64,
}

#[derive(Deserialize)]
pub(super) struct RegularResponse {
  choices: Vec<RegularChoice>,
  usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct RegularChoice {
  message: RegularMessage,
}

#[derive(Deserialize)]
struct RegularMessage {
  content: Option<String>,
  #[serde(default)]
  tool_calls: Vec<RegularCall>,
}

#[derive(Deserialize)]
struct RegularCall {
  id: String,
  function: RegularFunction,
}

#[derive(Deserialize)]
struct RegularFunction {
  name: String,
  arguments: String,
}

pub(super) fn parse_response(
  response: RegularResponse,
  events: &EventSink,
) -> Result<ProviderReply> {
  let choice = response
    .choices
    .into_iter()
    .next()
    .context("model response had no choices")?;
  if let Some(text) = &choice.message.content {
    events.emit(Event::TextDelta { text: text.clone() });
  }
  let tool_calls = choice
    .message
    .tool_calls
    .into_iter()
    .map(|call| {
      Ok(ToolCall {
        id: call.id,
        name: call.function.name,
        arguments: serde_json::from_str(&call.function.arguments)?,
      })
    })
    .collect::<Result<_>>()?;
  let usage = response
    .usage
    .map(|usage| Usage {
      input_tokens: usage.prompt_tokens,
      output_tokens: usage.completion_tokens,
      cost_usd: None,
    })
    .unwrap_or_default();
  Ok(ProviderReply {
    message: Message {
      role: Role::Assistant,
      content: choice.message.content,
      tool_calls,
      tool_call_id: None,
      images: Vec::new(),
    },
    usage,
  })
}

pub(super) fn parse_data(
  data: &str,
  content: &mut String,
  calls: &mut BTreeMap<usize, PartialCall>,
  usage: &mut Usage,
  events: &EventSink,
) -> Result<()> {
  let data = data.trim();
  if data == "[DONE]" {
    return Ok(());
  }
  let chunk: StreamChunk = serde_json::from_str(data)
    .with_context(|| format!("invalid stream event: {}", excerpt(data)))?;
  if let Some(wire) = chunk.usage {
    usage.input_tokens = wire.prompt_tokens;
    usage.output_tokens = wire.completion_tokens;
  }
  for choice in chunk.choices {
    if let Some(text) = choice.delta.content {
      content.push_str(&text);
      events.emit(Event::TextDelta { text });
    }
    for delta in choice.delta.tool_calls {
      let call = calls.entry(delta.index).or_default();
      if let Some(id) = delta.id {
        call.id = id;
      }
      if let Some(function) = delta.function {
        if let Some(name) = function.name {
          call.name.push_str(&name);
        }
        if let Some(arguments) = function.arguments {
          call.arguments.push_str(&arguments);
        }
      }
    }
  }
  Ok(())
}

fn excerpt(data: &str) -> String {
  data.chars().take(200).collect()
}
