//! The Anthropic Messages API. Content is a list of blocks rather than a string, tool
//! results go back as user blocks, and thinking comes with a signature that has to return
//! verbatim, so the conversion is a real translation rather than a rename.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ChatProvider, ProviderReply, Wire, arguments, default_header, read_events};
use crate::{
  event::{Event, EventSink},
  protocol::{Message, Role, Thinking, ToolCall, ToolSpec, Usage},
};

#[derive(Clone)]
pub struct AnthropicProvider {
  wire: Wire,
  thinking_tokens: usize,
}

impl AnthropicProvider {
  pub fn new(
    endpoint: String,
    model: String,
    api_key: Option<String>,
    mut headers: BTreeMap<String, String>,
    thinking_tokens: usize,
    retries: usize,
  ) -> Result<Self> {
    default_header(&mut headers, "anthropic-version", "2023-06-01".into());
    if let Some(key) = api_key {
      default_header(&mut headers, "x-api-key", key);
    }
    Ok(Self {
      wire: Wire::new(&endpoint, model, headers, retries)?,
      thinking_tokens,
    })
  }

  pub(crate) fn with_model(&self, model: String) -> Self {
    Self {
      wire: self.wire.with_model(model),
      ..self.clone()
    }
  }

  pub async fn models(&self) -> Result<Vec<String>> {
    self.wire.models("models?limit=100").await
  }
}

#[async_trait]
impl ChatProvider for AnthropicProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    let (system, conversation) = split(messages);
    let mut body = json!({
      "model": self.wire.model,
      "max_tokens": max_tokens(self.thinking_tokens),
      "messages": conversation,
      "stream": true,
    });
    if !system.is_empty() {
      body["system"] = json!(system);
    }
    if !tools.is_empty() {
      body["tools"] = tools
        .iter()
        .map(|tool| {
          json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.parameters,
          })
        })
        .collect();
    }
    if self.thinking_tokens > 0 {
      body["thinking"] = json!({"type": "enabled", "budget_tokens": self.thinking_tokens});
    }
    let response = self.wire.post("messages", &body, events).await?;
    let mut state = StreamState::default();
    read_events(response, |data| state.push(data, events)).await?;
    state.finish()
  }
}

/// The thinking budget counts against `max_tokens` and must be below it. A budget that already
/// fills the usual ceiling gets room for a reply on top instead of a 400 on every request;
/// anything smaller keeps the ceiling that models with a 32k output limit accept.
fn max_tokens(thinking: usize) -> usize {
  const CEILING: usize = 32_000;
  if thinking < CEILING {
    CEILING
  } else {
    thinking + 16_000
  }
}

/// Anthropic takes the system prompt out of band, and the rest as alternating turns.
fn split(messages: &[&Message]) -> (String, Vec<Value>) {
  let mut system = Vec::new();
  let mut conversation: Vec<Value> = Vec::new();
  for message in messages {
    match message.role {
      Role::System => {
        if let Some(text) = &message.content {
          system.push(text.clone());
        }
      }
      Role::Tool => {
        let block = json!({
          "type": "tool_result",
          "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
          "content": message.content.clone().unwrap_or_default(),
        });
        // consecutive results belong to one user turn, which is what the API expects back
        match conversation.last_mut() {
          Some(last) if last["role"] == "user" && last["content"][0]["type"] == "tool_result" => {
            if let Some(blocks) = last["content"].as_array_mut() {
              blocks.push(block);
            }
          }
          _ => conversation.push(json!({"role": "user", "content": [block]})),
        }
      }
      Role::User => {
        let mut blocks = Vec::new();
        for image in &message.images {
          if let Some((media_type, data)) = image.parts() {
            blocks.push(json!({
              "type": "image",
              "source": {"type": "base64", "media_type": media_type, "data": data},
            }));
          } else {
            blocks.push(json!({"type": "image", "source": {"type": "url", "url": image.url}}));
          }
        }
        if let Some(text) = message.content.as_deref().filter(|text| !text.is_empty()) {
          blocks.push(json!({"type": "text", "text": text}));
        }
        if !blocks.is_empty() {
          conversation.push(json!({"role": "user", "content": blocks}));
        }
      }
      Role::Assistant => {
        let mut blocks = Vec::new();
        if let Some(thinking) = &message.thinking {
          if !thinking.text.is_empty() {
            let mut block = json!({"type": "thinking", "thinking": thinking.text});
            if let Some(signature) = &thinking.signature {
              block["signature"] = json!(signature);
            }
            blocks.push(block);
          }
          for data in &thinking.redacted {
            blocks.push(json!({"type": "redacted_thinking", "data": data}));
          }
        }
        if let Some(text) = message.content.as_deref().filter(|text| !text.is_empty()) {
          blocks.push(json!({"type": "text", "text": text}));
        }
        for call in &message.tool_calls {
          blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.arguments,
          }));
        }
        if !blocks.is_empty() {
          conversation.push(json!({"role": "assistant", "content": blocks}));
        }
      }
    }
  }
  (system.join("\n\n"), conversation)
}

#[derive(Default)]
struct StreamState {
  text: String,
  thinking: String,
  signature: Option<String>,
  redacted: Vec<String>,
  calls: BTreeMap<usize, (String, String, String)>,
  usage: Usage,
  finished: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
  MessageStart {
    message: WireMessage,
  },
  MessageDelta {
    delta: Option<Stop>,
    usage: Option<WireUsage>,
  },
  ContentBlockStart {
    index: usize,
    content_block: Block,
  },
  ContentBlockDelta {
    index: usize,
    delta: Change,
  },
  MessageStop,
  Error {
    error: WireError,
  },
  #[serde(other)]
  Other,
}

#[derive(Deserialize)]
struct Stop {
  stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
  usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireError {
  message: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
  ToolUse {
    id: String,
    name: String,
  },
  RedactedThinking {
    data: String,
  },
  #[serde(other)]
  Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Change {
  TextDelta {
    text: String,
  },
  ThinkingDelta {
    thinking: String,
  },
  SignatureDelta {
    signature: String,
  },
  InputJsonDelta {
    partial_json: String,
  },
  #[serde(other)]
  Other,
}

#[derive(Deserialize)]
struct WireUsage {
  input_tokens: Option<u64>,
  output_tokens: Option<u64>,
  cache_read_input_tokens: Option<u64>,
  cache_creation_input_tokens: Option<u64>,
  #[serde(alias = "cost")]
  cost_usd: Option<f64>,
}

impl StreamState {
  /// True once the message is complete and nothing after it needs reading.
  fn push(&mut self, data: &str, events: &EventSink) -> Result<bool> {
    let data = data.trim();
    if data.is_empty() {
      return Ok(false);
    }
    let event: WireEvent = serde_json::from_str(data)
      .with_context(|| format!("invalid stream event: {}", super::excerpt(data)))?;
    match event {
      WireEvent::MessageStop => self.finished = true,
      // an error mid-stream is the answer; waiting for a stop that never comes would bury it
      WireEvent::Error { error } => bail!(
        "provider reported an error: {}",
        error
          .message
          .as_deref()
          .unwrap_or("the provider reported an error")
      ),
      WireEvent::MessageDelta {
        delta: Some(Stop {
          stop_reason: Some(reason),
        }),
        ..
      } if matches!(reason.as_str(), "max_tokens" | "refusal") => {
        bail!("model reply ended with {reason}")
      }
      WireEvent::MessageStart {
        message: WireMessage { usage: Some(usage) },
      }
      | WireEvent::MessageDelta {
        usage: Some(usage), ..
      } => self.count(usage),
      WireEvent::ContentBlockStart {
        index,
        content_block,
      } => match content_block {
        Block::ToolUse { id, name } => {
          self.calls.insert(index, (id, name, String::new()));
        }
        Block::RedactedThinking { data } => self.redacted.push(data),
        Block::Other => {}
      },
      WireEvent::ContentBlockDelta { index, delta } => match delta {
        Change::TextDelta { text } => {
          self.text.push_str(&text);
          events.emit(Event::TextDelta { text });
        }
        Change::ThinkingDelta { thinking } => {
          self.thinking.push_str(&thinking);
          events.emit(Event::ThinkingDelta { text: thinking });
        }
        Change::SignatureDelta { signature } => {
          self.signature.get_or_insert_default().push_str(&signature)
        }
        Change::InputJsonDelta { partial_json } => {
          if let Some((_, _, input)) = self.calls.get_mut(&index) {
            input.push_str(&partial_json);
          }
        }
        Change::Other => {}
      },
      _ => {}
    }
    Ok(self.finished)
  }

  /// Each figure is the running total so far, and a zero is a field this event did not carry.
  fn count(&mut self, usage: WireUsage) {
    let set = |slot: &mut u64, value: Option<u64>| {
      if let Some(value) = value.filter(|value| *value > 0) {
        *slot = value;
      }
    };
    if let Some(input) = usage.input_tokens {
      self.usage.input_tokens = input
        + usage.cache_read_input_tokens.unwrap_or(0)
        + usage.cache_creation_input_tokens.unwrap_or(0);
    }
    set(&mut self.usage.output_tokens, usage.output_tokens);
    if usage.cost_usd.is_some() {
      self.usage.cost_usd = usage.cost_usd;
    }
  }

  fn finish(self) -> Result<ProviderReply> {
    if !self.finished {
      bail!("model stream ended before completion");
    }
    let tool_calls = self
      .calls
      .into_values()
      .map(|(id, name, input)| {
        Ok(ToolCall {
          arguments: arguments(&input),
          id,
          name,
        })
      })
      .collect::<Result<Vec<_>>>()?;
    let thinking = (!self.thinking.is_empty() || !self.redacted.is_empty()).then_some(Thinking {
      text: self.thinking,
      signature: self.signature,
      redacted: self.redacted,
    });
    Ok(ProviderReply {
      message: Message::assistant(
        (!self.text.is_empty()).then_some(self.text),
        thinking,
        tool_calls,
      ),
      usage: self.usage,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn replay(lines: &[&str]) -> Result<ProviderReply> {
    let mut state = StreamState::default();
    for line in lines {
      state.push(line, &EventSink::default())?;
    }
    state.finish()
  }

  #[test]
  fn an_error_mid_stream_is_reported_rather_than_a_missing_stop() {
    let mut state = StreamState::default();
    let error = state
      .push(
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        &EventSink::default(),
      )
      .expect_err("an error event ends the reply");
    assert!(error.to_string().contains("Overloaded"), "{error}");
  }

  #[test]
  fn parallel_tool_calls_keep_their_own_arguments() {
    let reply = replay(&[
      r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"a","name":"read","input":{}}}"#,
      r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"b","name":"list","input":{}}}"#,
      r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
      r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"x\"}"}}"#,
      r#"{"type":"message_stop"}"#,
    ])
    .unwrap();
    let calls = &reply.message.tool_calls;
    assert_eq!(calls[0].arguments, json!({"path": "x"}));
    assert_eq!(calls[1].name, "list");
    assert_eq!(calls[1].arguments, json!({}));
  }

  #[test]
  fn redacted_thinking_goes_back_with_the_call_it_preceded() {
    let reply = replay(&[
      r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"sealed"}}"#,
      r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"a","name":"read","input":{}}}"#,
      r#"{"type":"message_stop"}"#,
    ])
    .unwrap();
    let (_, conversation) = split(&[&reply.message]);
    let blocks = &conversation[0]["content"];
    assert_eq!(blocks[0]["type"], "redacted_thinking");
    assert_eq!(blocks[0]["data"], "sealed");
    assert_eq!(blocks[1]["type"], "tool_use");
  }

  #[test]
  fn a_thinking_budget_above_the_ceiling_still_leaves_room_to_answer() {
    assert_eq!(max_tokens(0), 32_000);
    assert_eq!(max_tokens(10_000), 32_000);
    assert!(max_tokens(32_000) > 32_000);
    assert!(max_tokens(60_000) > 60_000);
  }

  #[test]
  fn usage_keeps_the_input_count_when_the_final_delta_omits_it() {
    let reply = replay(&[
      r#"{"type":"message_start","message":{"usage":{"input_tokens":120,"output_tokens":1,"cache_read_input_tokens":null}}}"#,
      r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#,
      r#"{"type":"message_stop"}"#,
    ])
    .unwrap();
    assert_eq!(reply.usage.input_tokens, 120);
    assert_eq!(reply.usage.output_tokens, 42);
  }
}
