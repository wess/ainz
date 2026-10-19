use std::{borrow::Cow, collections::BTreeMap};

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::{Value, json};

use super::ProviderReply;
use crate::{
  event::{Event, EventSink},
  protocol::{INVALID_ARGUMENTS, Message, Role, ToolCall, ToolSpec, Usage},
};

/// A chat completions request, borrowed from the session. A run sends one on every step, so
/// it is serialized straight from the messages rather than rebuilt as a JSON tree first.
#[derive(Serialize)]
pub(super) struct ChatRequest<'a> {
  pub model: &'a str,
  pub messages: Vec<WireMessage<'a>>,
  pub stream: bool,
  pub stream_options: StreamOptions,
  // an empty tools array with tool_choice is rejected by several OpenAI-compatible servers
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub tools: Vec<WireTool<'a>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tool_choice: Option<&'static str>,
}

#[derive(Serialize)]
pub(super) struct StreamOptions {
  pub include_usage: bool,
}

#[derive(Serialize)]
pub(super) struct WireTool<'a> {
  #[serde(rename = "type")]
  kind: &'static str,
  function: &'a ToolSpec,
}

impl<'a> WireTool<'a> {
  pub(super) fn new(function: &'a ToolSpec) -> Self {
    Self {
      kind: "function",
      function,
    }
  }
}

#[derive(Serialize)]
pub(super) struct WireMessage<'a> {
  role: &'static str,
  content: WireContent<'a>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  tool_calls: Vec<WireCall<'a>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum WireContent<'a> {
  Text(Option<&'a str>),
  Parts(Vec<WirePart<'a>>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WirePart<'a> {
  Text { text: &'a str },
  ImageUrl { image_url: WireImage<'a> },
}

#[derive(Serialize)]
struct WireImage<'a> {
  url: &'a str,
  #[serde(skip_serializing_if = "Option::is_none")]
  detail: Option<&'a str>,
}

#[derive(Serialize)]
struct WireCall<'a> {
  id: &'a str,
  #[serde(rename = "type")]
  kind: &'static str,
  function: WireFunction<'a>,
}

#[derive(Serialize)]
struct WireFunction<'a> {
  name: &'a str,
  // the wire carries arguments as a string of JSON, not as JSON
  arguments: String,
}

pub(super) fn wire_message(message: &Message) -> WireMessage<'_> {
  let role = match message.role {
    Role::System => "system",
    Role::User => "user",
    Role::Assistant => "assistant",
    Role::Tool => "tool",
  };
  let content = if message.images.is_empty() {
    WireContent::Text(message.content.as_deref())
  } else {
    let text = message
      .content
      .as_deref()
      .map(|text| WirePart::Text { text });
    let images = message.images.iter().map(|image| WirePart::ImageUrl {
      image_url: WireImage {
        url: &image.url,
        detail: image.detail.as_deref(),
      },
    });
    WireContent::Parts(text.into_iter().chain(images).collect())
  };
  let tool_calls = message
    .tool_calls
    .iter()
    .map(|call| WireCall {
      id: &call.id,
      kind: "function",
      function: WireFunction {
        name: &call.name,
        arguments: call.arguments.to_string(),
      },
    })
    .collect();
  WireMessage {
    role,
    content,
    tool_calls,
    tool_call_id: message.tool_call_id.as_deref(),
  }
}

#[derive(Default)]
pub(super) struct PartialCall {
  id: String,
  name: String,
  arguments: String,
}

impl PartialCall {
  pub(super) fn finish(self) -> Result<ToolCall> {
    if self.id.is_empty() || self.name.is_empty() {
      bail!("model returned an incomplete tool call");
    }
    Ok(ToolCall {
      id: self.id,
      name: self.name,
      arguments: arguments(self.arguments),
    })
  }
}

// a call that takes nothing may stream nothing at all. arguments that will not parse are kept
// rather than failing the reply: the reply is still history, and the model can only correct a
// call it is told about
fn arguments(raw: String) -> Value {
  if raw.trim().is_empty() {
    return json!({});
  }
  match serde_json::from_str::<Value>(&raw) {
    Ok(value @ Value::Object(_)) => value,
    _ => json!({ INVALID_ARGUMENTS: raw }),
  }
}

// said after a reply the output limit cut short, so neither the person nor the model takes what
// is there for the whole answer
const CUT_OFF: &str = "\n\n[cut off: this reply reached its length limit]";

// compatible servers send null where OpenAI leaves a field out, hence the Options throughout;
// strings that are only copied into an accumulator are borrowed from the event instead
#[derive(Deserialize)]
struct StreamChunk<'a> {
  #[serde(borrow)]
  choices: Option<Vec<Choice<'a>>>,
  usage: Option<WireUsage>,
  error: Option<IgnoredAny>,
  #[serde(rename = "type", borrow)]
  kind: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Choice<'a> {
  #[serde(borrow)]
  delta: Option<Delta<'a>>,
  #[serde(borrow)]
  finish_reason: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Delta<'a> {
  content: Option<String>,
  #[serde(borrow)]
  tool_calls: Option<Vec<CallDelta<'a>>>,
}

#[derive(Deserialize)]
struct CallDelta<'a> {
  index: Option<usize>,
  #[serde(borrow)]
  id: Option<Cow<'a, str>>,
  #[serde(borrow)]
  function: Option<FunctionDelta<'a>>,
}

#[derive(Deserialize)]
struct FunctionDelta<'a> {
  #[serde(borrow)]
  name: Option<Cow<'a, str>>,
  #[serde(borrow)]
  arguments: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct WireUsage {
  prompt_tokens: Option<u64>,
  completion_tokens: Option<u64>,
  #[serde(alias = "cost")]
  cost_usd: Option<f64>,
}

impl From<WireUsage> for Usage {
  fn from(wire: WireUsage) -> Self {
    Self {
      input_tokens: wire.prompt_tokens.unwrap_or(0),
      output_tokens: wire.completion_tokens.unwrap_or(0),
      cost_usd: wire.cost_usd,
    }
  }
}

#[derive(Deserialize)]
struct RegularResponse {
  choices: Option<Vec<RegularChoice>>,
  usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct RegularChoice {
  message: RegularMessage,
  finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct RegularMessage {
  content: Option<String>,
  tool_calls: Option<Vec<RegularCall>>,
}

#[derive(Deserialize)]
struct RegularCall {
  id: String,
  function: RegularFunction,
}

#[derive(Deserialize)]
struct RegularFunction {
  name: String,
  arguments: Option<String>,
}

pub(super) fn parse_response(body: &[u8], events: &EventSink) -> Result<ProviderReply> {
  let value: Value = serde_json::from_slice(body).context("invalid model response")?;
  check_error(&value)?;
  let response = RegularResponse::deserialize(value).context("invalid model response")?;
  let choice = response
    .choices
    .into_iter()
    .flatten()
    .next()
    .context("model response had no choices")?;
  let mut content = choice.message.content;
  match choice.finish_reason.as_deref() {
    Some("content_filter") => bail!("model reply ended with content_filter"),
    Some("length") => content.get_or_insert_default().push_str(CUT_OFF),
    _ => {}
  }
  if let Some(text) = &content {
    events.emit(Event::TextDelta { text: text.clone() });
  }
  let tool_calls = choice
    .message
    .tool_calls
    .into_iter()
    .flatten()
    .map(|call| ToolCall {
      id: call.id,
      name: call.function.name,
      arguments: arguments(call.function.arguments.unwrap_or_default()),
    })
    .collect();
  Ok(ProviderReply {
    message: Message {
      role: Role::Assistant,
      content,
      tool_calls,
      tool_call_id: None,
      images: Vec::new(),
      thinking: None,
      response_items: Vec::new(),
      response_model: None,
    },
    usage: response.usage.map(Usage::from).unwrap_or_default(),
  })
}

// true once the reply is complete: a finish reason or [DONE]. a reason this does not know, such
// as a gateway's end_turn, still ends the reply; only a filtered one is refused, because what is
// missing from it was removed on purpose. a reply cut off at the output limit is kept and marked,
// and a call it was in the middle of reaches the model as a call whose arguments did not survive
pub(super) fn parse_data(
  data: &str,
  content: &mut String,
  calls: &mut BTreeMap<usize, PartialCall>,
  usage: &mut Usage,
  events: &EventSink,
) -> Result<bool> {
  let data = data.trim();
  if data == "[DONE]" {
    return Ok(true);
  }
  if data.is_empty() {
    return Ok(false);
  }
  let chunk: StreamChunk = match serde_json::from_str(data) {
    Ok(chunk) => chunk,
    Err(error) => {
      // an error object whose fields clash with a chunk's shape still deserves its sentence
      if let Ok(value) = serde_json::from_str::<Value>(data) {
        check_error(&value)?;
      }
      return Err(error).with_context(|| format!("invalid stream event: {}", excerpt(data)));
    }
  };
  if chunk.error.is_some() || chunk.kind.as_deref() == Some("error") {
    bail!("model reply failed: {}", error_message(data));
  }
  if let Some(wire) = chunk.usage {
    *usage = wire.into();
  }
  let mut finished = false;
  for choice in chunk.choices.into_iter().flatten() {
    let reason = choice.finish_reason.as_deref();
    if reason == Some("content_filter") {
      bail!("model reply ended with content_filter");
    }
    // the last piece of text can share its event with the finish reason, and comes before it
    if let Some(delta) = choice.delta {
      if let Some(text) = delta.content.filter(|text| !text.is_empty()) {
        content.push_str(&text);
        events.emit(Event::TextDelta { text });
      }
      for delta in delta.tool_calls.into_iter().flatten() {
        call(calls, delta);
      }
    }
    if reason == Some("length") {
      content.push_str(CUT_OFF);
      events.emit(Event::TextDelta {
        text: CUT_OFF.into(),
      });
    }
    finished |= reason.is_some();
  }
  Ok(finished)
}

// servers that leave out `index` send each call whole, so a new id is a new call rather than
// more of the last one
fn call(calls: &mut BTreeMap<usize, PartialCall>, delta: CallDelta) {
  let index = delta
    .index
    .unwrap_or_else(|| match (calls.last_key_value(), delta.id.as_deref()) {
      (Some((index, last)), Some(id)) if last.id != id => index + 1,
      (Some((index, _)), _) => *index,
      (None, _) => 0,
    });
  let call = calls.entry(index).or_default();
  if let Some(id) = delta.id {
    call.id = id.into_owned();
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

fn check_error(value: &Value) -> Result<()> {
  if value.get("error").is_some_and(|error| !error.is_null()) || value["type"] == "error" {
    bail!("model reply failed: {}", error_message(&value.to_string()));
  }
  Ok(())
}

// what a provider said went wrong. these bodies are JSON, and a person reading a failed turn
// should get the sentence in it rather than the punctuation around it
pub(crate) fn wire_error(status: StatusCode, body: &str) -> String {
  let sentence = error_message(body);
  if sentence.is_empty() {
    status.to_string()
  } else {
    format!("{status}: {sentence}")
  }
}

pub(super) fn error_message(body: &str) -> String {
  let sentence = match serde_json::from_str::<Value>(body.trim()) {
    Ok(value) => ["/error/message", "/message", "/detail", "/error"]
      .iter()
      .find_map(|path| value.pointer(path).and_then(Value::as_str))
      .or_else(|| value.as_str())
      .map(str::trim)
      .filter(|text| !text.is_empty())
      .unwrap_or("provider returned an error without a message")
      .to_string(),
    Err(_) => body.trim().chars().take(300).collect(),
  };
  // gateways pass an upstream's error along as text inside their own message
  if let Some(start) = sentence.find("{\"error\"") {
    let mut values = serde_json::Deserializer::from_str(&sentence[start..]).into_iter::<Value>();
    if let Some(Ok(value)) = values.next()
      && let Some(message) = value.pointer("/error/message").and_then(Value::as_str)
    {
      let prefix = sentence[..start].trim().trim_end_matches(['-', ':']).trim();
      return if prefix.is_empty() {
        message.to_string()
      } else {
        format!("{prefix}: {message}")
      };
    }
  }
  sentence
}

fn excerpt(data: &str) -> String {
  data.chars().take(200).collect()
}

#[cfg(test)]
#[path = "../../tests/provider/wire.rs"]
mod tests;
