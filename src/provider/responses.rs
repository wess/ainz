use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
  ChatProvider, MAX_TRANSFER, ProviderReply, Wire, arguments, default_header, error_message,
  read_events,
};
use crate::{
  event::{Event, EventSink},
  protocol::{Message, Role, ToolCall, ToolSpec, Usage},
};

#[derive(Clone)]
pub struct ResponsesProvider {
  wire: Wire,
  reasoning_effort: Option<String>,
}

impl ResponsesProvider {
  pub fn new(
    endpoint: String,
    model: String,
    api_key: Option<String>,
    mut headers: BTreeMap<String, String>,
    retries: usize,
    reasoning_effort: Option<String>,
  ) -> Result<Self> {
    if let Some(key) = api_key {
      default_header(&mut headers, "authorization", format!("Bearer {key}"));
    }
    Ok(Self {
      wire: Wire::new(&endpoint, model, headers, retries)?,
      reasoning_effort,
    })
  }

  pub(crate) fn with_model(&self, model: String) -> Self {
    Self {
      wire: self.wire.with_model(model),
      ..self.clone()
    }
  }

  pub async fn models(&self) -> Result<Vec<String>> {
    self.wire.models("models").await
  }
}

#[async_trait]
impl ChatProvider for ResponsesProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    let mut body = json!({
      "model": self.wire.model,
      "input": input(messages, &self.wire.model)?,
      "stream": true,
      "store": false,
      "include": ["reasoning.encrypted_content"],
    });
    if !tools.is_empty() {
      body["tools"] = tools
        .iter()
        .map(|tool| {
          json!({
            "type": "function", "name": tool.name, "description": tool.description,
            "parameters": tool.parameters, "strict": false,
          })
        })
        .collect();
      body["tool_choice"] = json!("auto");
    }
    if let Some(effort) = &self.reasoning_effort {
      body["reasoning"] = json!({"effort": effort});
    }
    let response = self.wire.post("responses", &body, events).await?;
    let streaming = response
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|value| value.to_str().ok())
      .is_some_and(|value| value.contains("text/event-stream"));
    if !streaming {
      let value = serde_json::from_slice(&crate::output::response(response, MAX_TRANSFER).await?)
        .context("invalid Responses reply")?;
      return reply(value, &self.wire.model, events, false);
    }
    let mut completed = None;
    let mut streamed = false;
    read_events(response, |data| {
      let value: Value = serde_json::from_str(data).context("invalid Responses stream event")?;
      match value["type"].as_str() {
        Some("error" | "response.failed" | "response.incomplete") => {
          let failure = value.get("response").unwrap_or(&value);
          bail!("model stream failed: {}", failure_message(failure));
        }
        Some("response.output_text.delta" | "response.refusal.delta") => {
          if let Some(text) = value["delta"].as_str() {
            streamed = true;
            events.emit(Event::TextDelta { text: text.into() });
          }
        }
        Some("response.reasoning_summary_text.delta") => {
          if let Some(text) = value["delta"].as_str() {
            events.emit(Event::ThinkingDelta { text: text.into() });
          }
        }
        Some("response.completed") => {
          completed = Some(value["response"].clone());
          return Ok(true);
        }
        _ => {}
      }
      Ok(false)
    })
    .await?;
    reply(
      completed.context("model stream ended before completion")?,
      &self.wire.model,
      events,
      streamed,
    )
  }
}

fn input(messages: &[&Message], model: &str) -> Result<Vec<Value>> {
  let mut items = Vec::new();
  for message in messages {
    if message.role == Role::Tool {
      items.push(json!({"type": "function_call_output",
        "call_id": message.tool_call_id.as_deref().context("tool result has no call id")?,
        "output": message.content.as_deref().unwrap_or_default()}));
      continue;
    }
    // Encrypted reasoning is tied to the model that produced it.
    if message.role == Role::Assistant
      && message.response_model.as_deref() == Some(model)
      && !message.response_items.is_empty()
    {
      items.extend(message.response_items.iter().cloned());
      continue;
    }
    let mut content = Vec::new();
    if let Some(text) = &message.content {
      content.push(json!({"type": if message.role == Role::Assistant {"output_text"} else {"input_text"}, "text": text}));
    }
    for image in &message.images {
      let mut part = json!({"type": "input_image", "image_url": image.url});
      if let Some(detail) = &image.detail {
        part["detail"] = json!(detail);
      }
      content.push(part);
    }
    if !content.is_empty() {
      items.push(json!({"role": message.role.label(), "content": content}));
    }
    for call in &message.tool_calls {
      items.push(json!({"type": "function_call", "call_id": call.id,
        "name": call.name, "arguments": call.arguments.to_string()}));
    }
  }
  Ok(items)
}

fn reply(value: Value, model: &str, events: &EventSink, streamed: bool) -> Result<ProviderReply> {
  if value["status"].as_str() != Some("completed") {
    bail!("model reply failed: {}", failure_message(&value));
  }
  let output = value["output"]
    .as_array()
    .context("Responses reply has no output")?;
  let mut text = String::new();
  let mut calls = Vec::new();
  for item in output {
    if let Some(status) = item["status"].as_str()
      && status != "completed"
    {
      bail!("model output item is {status}");
    }
    match item["type"].as_str() {
      Some("message") => {
        for part in item["content"]
          .as_array()
          .context("message has no content")?
        {
          match part["type"].as_str() {
            Some("output_text") => {
              text.push_str(part["text"].as_str().context("output text is missing")?)
            }
            Some("refusal") => {
              text.push_str(part["refusal"].as_str().context("refusal is missing")?)
            }
            // a part this version has never seen is not worth losing the turn over
            _ => {}
          }
        }
      }
      Some("function_call") => {
        let name = item["name"].as_str().context("function call has no name")?;
        calls.push(ToolCall {
          id: item["call_id"]
            .as_str()
            .context("function call has no call id")?
            .into(),
          name: name.into(),
          arguments: arguments(item["arguments"].as_str().unwrap_or_default()),
        });
      }
      // reasoning, and any item type newer than this code, rides along in `response_items`
      _ => {}
    }
  }
  if text.is_empty() && calls.is_empty() {
    bail!("model reply had no answer or tool calls");
  }
  if !streamed && !text.is_empty() {
    events.emit(Event::TextDelta { text: text.clone() });
  }
  let mut message = Message::assistant((!text.is_empty()).then_some(text), None, calls);
  message.response_items = output.clone();
  message.response_model = Some(model.into());
  let usage = &value["usage"];
  let count = |key: &str| usage[key].as_u64().unwrap_or(0);
  Ok(ProviderReply {
    message,
    usage: Usage {
      input_tokens: count("input_tokens"),
      output_tokens: count("output_tokens"),
      cost_usd: usage["cost_usd"]
        .as_f64()
        .or_else(|| usage["cost"].as_f64()),
    },
  })
}

fn failure_message(value: &Value) -> String {
  if let Some(reason) = value
    .pointer("/incomplete_details/reason")
    .and_then(Value::as_str)
  {
    return format!("incomplete response ({reason})");
  }
  if value.get("error").is_some_and(|error| !error.is_null()) || value["message"].is_string() {
    return error_message(&value.to_string());
  }
  format!("response {}", value["status"].as_str().unwrap_or("failed"))
}
