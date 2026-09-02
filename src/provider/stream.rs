//! The line-by-line report a headless coding agent writes while it works, in the shape
//! `claude -p --output-format stream-json` produces: one JSON object per line, mixing token
//! deltas, whole messages, and a final result.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::{
  event::{Event, EventSink},
  protocol::{ToolCall, Usage},
  tool::truncate,
};

/// A reply read from a command, and whether it already reached the sink a piece at a time.
pub(super) struct Completion {
  pub text: String,
  pub usage: Usage,
  pub streamed: bool,
}

impl Completion {
  pub fn whole(text: String) -> Self {
    Self {
      text,
      usage: Usage::default(),
      streamed: false,
    }
  }
}

#[derive(Default)]
pub(super) struct StreamState {
  text: String,
  result: Option<String>,
  usage: Usage,
  failure: Option<String>,
}

impl StreamState {
  pub fn push(&mut self, line: &str, events: &EventSink) {
    // anything that is not a JSON object is the command talking to a human; skip it
    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
      return;
    };
    match field(&value, "type") {
      Some("stream_event") => self.delta(value.get("event"), events),
      Some("assistant") => tool_starts(&value, events),
      Some("user") => tool_ends(&value, events),
      Some("result") => self.result(&value),
      _ => {}
    }
  }

  fn delta(&mut self, event: Option<&Value>, events: &EventSink) {
    let Some(event) = event else { return };
    if field(event, "type") != Some("content_block_delta") {
      return;
    }
    let delta = &event["delta"];
    // thinking deltas arrive here too, and are the model's own working, not its answer
    if field(delta, "type") != Some("text_delta") {
      return;
    }
    let Some(text) = field(delta, "text") else {
      return;
    };
    self.text.push_str(text);
    events.emit(Event::TextDelta { text: text.into() });
  }

  fn result(&mut self, value: &Value) {
    if let Some(text) = field(value, "result") {
      self.result = Some(text.into());
    }
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
      let subtype = field(value, "subtype").unwrap_or("error");
      self.failure = Some(match self.result.as_deref() {
        Some(text) if !text.trim().is_empty() => text.trim().to_string(),
        _ => subtype.to_string(),
      });
    }
    let usage = &value["usage"];
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    // cached input still had to be read to answer, so it belongs in what the run cost
    self.usage = Usage {
      input_tokens: count("input_tokens")
        + count("cache_read_input_tokens")
        + count("cache_creation_input_tokens"),
      output_tokens: count("output_tokens"),
    };
  }

  pub fn finish(self) -> Result<Completion> {
    if let Some(failure) = self.failure {
      bail!("provider reported an error: {failure}");
    }
    if self.text.is_empty() {
      // no partial messages were asked for, so the final result is the whole reply
      return Ok(Completion {
        text: self.result.unwrap_or_default(),
        usage: self.usage,
        streamed: false,
      });
    }
    Ok(Completion {
      text: self.text,
      usage: self.usage,
      streamed: true,
    })
  }
}

fn tool_starts(value: &Value, events: &EventSink) {
  for block in blocks(value) {
    if field(block, "type") != Some("tool_use") {
      continue;
    }
    let (Some(id), Some(name)) = (field(block, "id"), field(block, "name")) else {
      continue;
    };
    events.emit(Event::ToolStart {
      call: ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: block.get("input").cloned().unwrap_or_default(),
      },
    });
  }
}

fn tool_ends(value: &Value, events: &EventSink) {
  for block in blocks(value) {
    if field(block, "type") != Some("tool_result") {
      continue;
    }
    let Some(id) = field(block, "tool_use_id") else {
      continue;
    };
    let output = match block.get("content") {
      Some(Value::String(text)) => text.clone(),
      Some(other) => other.to_string(),
      None => String::new(),
    };
    events.emit(Event::ToolEnd {
      id: id.into(),
      output: truncate(output, 2000),
      error: block.get("is_error").and_then(Value::as_bool) == Some(true),
    });
  }
}

fn blocks(value: &Value) -> impl Iterator<Item = &Value> {
  value["message"]["content"].as_array().into_iter().flatten()
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
  value.get(key).and_then(Value::as_str)
}
