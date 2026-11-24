//! Newline-delimited reports from headless commands. They carry messages, tool activity, worker
//! status, and a final result.

use std::io;

use anyhow::{Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use super::workers::Workers;
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

pub(super) struct StreamState {
  text: String,
  result: Option<String>,
  usage: Usage,
  failure: Option<String>,
  workers: Workers,
  exec: bool,
  /// The final result has arrived; a command still reading stdin can be let go.
  pub done: bool,
}

impl StreamState {
  pub fn new(events: EventSink) -> Self {
    Self {
      text: String::new(),
      result: None,
      usage: Usage::default(),
      failure: None,
      workers: Workers::new(events),
      exec: false,
      done: false,
    }
  }

  /// Reads one line, and hands back a request the command is now waiting on an answer to.
  pub fn push(&mut self, line: &str) -> Option<Value> {
    // anything that is not a JSON object is the command talking to a human; skip it
    let value = serde_json::from_str::<Value>(line.trim()).ok()?;
    match field(&value, "type") {
      Some("system") => self.workers.task(&value),
      Some("stream_event") => {
        if let Some(sink) = self.workers.sink_for(&value) {
          self.delta(
            value.get("event"),
            &sink,
            field(&value, "parent_tool_use_id").is_none(),
          );
        }
      }
      Some("assistant") => {
        self.workers.observe_calls(&value);
        if let Some(sink) = self.workers.sink_for(&value) {
          tool_starts(&value, &sink);
        }
      }
      Some("user") => {
        if let Some(sink) = self.workers.sink_for(&value) {
          tool_ends(&value, &sink);
        }
      }
      Some("result") => self.result(&value),
      Some("thread.started" | "turn.started") => self.exec = true,
      Some(kind @ ("item.started" | "item.updated" | "item.completed")) => {
        let item = &value["item"];
        match field(item, "type") {
          Some("collab_tool_call") => self.workers.collaboration(item),
          Some("agent_message") if kind == "item.completed" => {
            self.result = field(item, "text").map(str::to_owned)
          }
          _ => {}
        }
      }
      Some("turn.completed") => {
        self.done = true;
        self.usage.input_tokens = value["usage"]["input_tokens"].as_u64().unwrap_or(0);
        self.usage.output_tokens = value["usage"]["output_tokens"].as_u64().unwrap_or(0);
      }
      Some("turn.failed") => {
        self.done = true;
        self.failure = Some(
          field(&value["error"], "message")
            .unwrap_or("turn failed")
            .into(),
        );
      }
      Some("control_request") => return Some(value),
      _ => {}
    }
    None
  }

  fn delta(&mut self, event: Option<&Value>, events: &EventSink, primary: bool) {
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
    if primary {
      self.text.push_str(text);
    }
    events.emit(Event::TextDelta { text: text.into() });
  }

  fn result(&mut self, value: &Value) {
    self.done = true;
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
      // the only field here the model itself cannot be asked to estimate; take it verbatim
      // from the one provider that reports it, and leave it unset for everyone else
      cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
    };
  }

  pub fn failure(&self) -> Option<&str> {
    self.failure.as_deref()
  }

  pub fn finish(self) -> Result<Completion> {
    if let Some(failure) = self.failure {
      bail!("provider reported an error: {failure}");
    }
    if self.exec && !self.done {
      bail!("provider stream ended before the turn completed");
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
      // a result can also arrive as content blocks; read them rather than printing their JSON
      Some(Value::Array(blocks)) => blocks
        .iter()
        .filter_map(|block| {
          field(block, "text")
            .or_else(|| field(block, "tool_name"))
            .or_else(|| field(block, "name"))
            .or_else(|| field(block, "type"))
        })
        .collect::<Vec<_>>()
        .join("\n"),
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

pub(super) fn blocks(value: &Value) -> impl Iterator<Item = &Value> {
  value["message"]["content"].as_array().into_iter().flatten()
}

pub(super) fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
  value.get(key).and_then(Value::as_str)
}

/// The next line of a command's report, or nothing at the end of it. A line past `limit` comes
/// back empty with its bytes read and dropped: one report too long to keep is no reason to end
/// the work it describes.
pub(super) async fn next_line<R>(reader: &mut R, limit: usize) -> io::Result<Option<Vec<u8>>>
where
  R: AsyncBufRead + Unpin,
{
  let mut line = Vec::new();
  let read = (&mut *reader)
    .take(limit as u64 + 1)
    .read_until(b'\n', &mut line)
    .await?;
  if read == 0 {
    return Ok(None);
  }
  if line.len() <= limit {
    return Ok(Some(line));
  }
  if line.last() != Some(&b'\n') {
    skip_line(reader).await?;
  }
  Ok(Some(Vec::new()))
}

async fn skip_line<R>(reader: &mut R) -> io::Result<()>
where
  R: AsyncBufRead + Unpin,
{
  loop {
    let available = reader.fill_buf().await?;
    if available.is_empty() {
      return Ok(());
    }
    match available.iter().position(|byte| *byte == b'\n') {
      Some(end) => {
        reader.consume(end + 1);
        return Ok(());
      }
      None => {
        let length = available.len();
        reader.consume(length);
      }
    }
  }
}

#[cfg(test)]
mod lines {
  use super::next_line;

  #[tokio::test]
  async fn a_line_ending_on_the_limit_does_not_take_the_next_one_with_it() {
    let mut reader: &[u8] = b"abcd\nok\n";
    assert_eq!(next_line(&mut reader, 4).await.unwrap(), Some(Vec::new()));
    assert_eq!(
      next_line(&mut reader, 4).await.unwrap(),
      Some(b"ok\n".to_vec())
    );
    assert_eq!(next_line(&mut reader, 4).await.unwrap(), None);
  }

  #[tokio::test]
  async fn a_line_past_the_limit_is_dropped_up_to_its_end() {
    let mut reader: &[u8] = b"abcdefgh\nok\n";
    assert_eq!(next_line(&mut reader, 4).await.unwrap(), Some(Vec::new()));
    assert_eq!(
      next_line(&mut reader, 4).await.unwrap(),
      Some(b"ok\n".to_vec())
    );
  }

  #[tokio::test]
  async fn a_last_line_without_an_ending_is_still_a_line() {
    let mut reader: &[u8] = b"ok";
    assert_eq!(
      next_line(&mut reader, 4).await.unwrap(),
      Some(b"ok".to_vec())
    );
  }
}
