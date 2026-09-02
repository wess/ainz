use crate::protocol::{Message, ToolSpec};

pub fn estimate_tokens(instructions: &str, messages: &[Message], tools: &[ToolSpec]) -> usize {
  let message_bytes = messages
    .iter()
    .map(|message| {
      message.content.as_ref().map_or(0, String::len)
        + message
          .tool_calls
          .iter()
          .map(|call| call.name.len() + call.arguments.to_string().len() + call.id.len())
          .sum::<usize>()
        + message.tool_call_id.as_ref().map_or(0, String::len)
        + message.images.len() * 4_096
        + 12
    })
    .sum::<usize>();
  let tool_bytes = tools
    .iter()
    .map(|tool| tool.name.len() + tool.description.len() + tool.parameters.to_string().len())
    .sum::<usize>();
  (instructions.len() + message_bytes + tool_bytes).div_ceil(4)
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
