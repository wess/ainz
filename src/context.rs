use std::io::{self, Write};

use crate::protocol::{Message, ToolSpec};

/// A run checks this before every request, so it measures the context in place: nothing is
/// serialized into a string just to be counted and dropped.
pub fn estimate_tokens<'a>(
  instructions: &str,
  messages: impl IntoIterator<Item = &'a Message>,
  tools: &[ToolSpec],
) -> usize {
  let message_bytes = messages.into_iter().map(message_bytes).sum::<usize>();
  (instructions.len() + message_bytes + tool_bytes(tools)).div_ceil(4)
}

/// What the tool list costs on every request. It does not change during a run, so the run
/// measures it once.
pub fn tool_bytes(tools: &[ToolSpec]) -> usize {
  tools
    .iter()
    .map(|tool| tool.name.len() + tool.description.len() + json_len(&tool.parameters))
    .sum()
}

pub fn message_bytes(message: &Message) -> usize {
  message.content.as_ref().map_or(0, String::len)
    + message
      .tool_calls
      .iter()
      .map(|call| call.name.len() + json_len(&call.arguments) + call.id.len())
      .sum::<usize>()
    + message.tool_call_id.as_ref().map_or(0, String::len)
    + message.thinking.as_ref().map_or(0, |thinking| {
      thinking.text.len()
        + thinking.signature.as_ref().map_or(0, String::len)
        + thinking.redacted.iter().map(String::len).sum::<usize>()
    })
    + message.response_items.iter().map(json_len).sum::<usize>()
    + message.images.len() * 4_096
    + 12
}

struct Count(usize);

impl Write for Count {
  fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
    self.0 += bytes.len();
    Ok(bytes.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

fn json_len(value: &serde_json::Value) -> usize {
  let mut count = Count(0);
  drop(serde_json::to_writer(&mut count, value));
  count.0
}

pub fn transcript(messages: &[Message]) -> String {
  messages
    .iter()
    .map(|message| {
      let role = format!("{:?}", message.role).to_lowercase();
      let mut line = format!("{role}: {}", message.content.as_deref().unwrap_or(""));
      if !message.tool_calls.is_empty() {
        line.push_str("\ntool calls: ");
        line.push_str(&serde_json::to_string(&message.tool_calls).unwrap_or_default());
      }
      if let Some(id) = &message.tool_call_id {
        line.push_str(&format!("\ntool result for: {id}"));
      }
      for image in &message.images {
        let kind = image
          .url
          .strip_prefix("data:")
          .and_then(|value| value.split_once(';'))
          .map_or("image", |(kind, _)| kind);
        line.push_str(&format!("\n[attached {kind}]"));
      }
      line
    })
    .collect::<Vec<_>>()
    .join("\n\n")
}
