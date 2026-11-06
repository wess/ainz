//! What a process provider is handed on stdin: the conversation as one text prompt, wrapped as
//! a single stream-json user message when the command was told to read that.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::protocol::{Message, Role};

/// The args are what tell the command how to read stdin, so they decide what it is sent.
pub(super) fn reads_json(args: &[String]) -> bool {
  args.iter().any(|arg| arg == "--input-format=stream-json")
    || args
      .windows(2)
      .any(|pair| pair[0] == "--input-format" && pair[1] == "stream-json")
}

pub(super) fn render(messages: &[&Message], command: &str, json: bool) -> Result<String> {
  let prompt = transcript(messages, command, json)?;
  if !json {
    return Ok(prompt);
  }
  let mut content = vec![json!({"type": "text", "text": prompt})];
  content.extend(
    messages
      .iter()
      .flat_map(|message| &message.images)
      .filter_map(|image| image.parts())
      .map(image_block),
  );
  let line = json!({"type": "user", "message": {"role": "user", "content": content}});
  Ok(format!("{line}\n"))
}

fn image_block((media_type, data): (&str, &str)) -> Value {
  json!({
    "type": "image",
    "source": {"type": "base64", "media_type": media_type, "data": data},
  })
}

// an image only survives a text prompt if it is something the command can fetch for itself:
// inline data has nowhere to go, and dropping it silently would answer a question about a
// picture nobody sent. only the newest message refuses, because the session keeps the turn
// that failed and an older image would otherwise fail every turn after it
fn transcript(messages: &[&Message], command: &str, json: bool) -> Result<String> {
  let mut prompt = String::new();
  for (index, message) in messages.iter().enumerate() {
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
      if !image.url.starts_with("data:") {
        prompt.push_str(&format!("[image: {}]\n", image.url));
      } else if json && image.parts().is_some() {
        prompt.push_str("[image attached]\n");
      } else if index + 1 == messages.len() {
        bail!(
          "{command} takes a text prompt, so the image in this turn cannot be sent. Ask about \
           it with a provider that accepts images, or give the command the file path instead."
        );
      } else {
        prompt.push_str(&format!(
          "[image: not sent, {command} takes a text prompt]\n"
        ));
      }
    }
    prompt.push('\n');
  }
  Ok(prompt)
}
