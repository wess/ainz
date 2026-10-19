use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
  System,
  User,
  Assistant,
  Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Message {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub thinking: Option<Thinking>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub response_items: Vec<Value>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub response_model: Option<String>,
  pub role: Role,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub content: Option<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub tool_calls: Vec<ToolCall>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub tool_call_id: Option<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub images: Vec<Image>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Thinking {
  pub text: String,
  /// Anthropic returns a signature that has to come back verbatim on the next request.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub signature: Option<String>,
  /// Thinking the provider encrypted rather than showed. It still has to go back with the
  /// tool call it preceded, or the next request is refused.
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub redacted: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Image {
  pub url: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

impl Image {
  pub fn parts(&self) -> Option<(&str, &str)> {
    self.url.strip_prefix("data:")?.split_once(";base64,")
  }

  pub async fn from_path(path: &Path) -> Result<Self> {
    let media_type = match path
      .extension()
      .and_then(|extension| extension.to_str())
      .map(str::to_ascii_lowercase)
      .as_deref()
    {
      Some("png") => "image/png",
      Some("jpg" | "jpeg") => "image/jpeg",
      Some("gif") => "image/gif",
      Some("webp") => "image/webp",
      _ => bail!("unsupported image type: {}", path.display()),
    };
    let size = tokio::fs::metadata(path)
      .await
      .with_context(|| format!("read image {}", path.display()))?
      .len();
    if size > 20 * 1024 * 1024 {
      bail!("image exceeds the 20 MiB limit: {}", path.display());
    }
    let data = tokio::fs::read(path)
      .await
      .with_context(|| format!("read image {}", path.display()))?;
    Ok(Self {
      url: format!("data:{media_type};base64,{}", STANDARD.encode(data)),
      detail: None,
    })
  }
}

impl Role {
  pub fn label(self) -> &'static str {
    match self {
      Self::System => "system",
      Self::User => "user",
      Self::Assistant => "assistant",
      Self::Tool => "tool",
    }
  }
}

impl Message {
  fn empty(role: Role) -> Self {
    Self {
      role,
      content: None,
      thinking: None,
      tool_calls: Vec::new(),
      tool_call_id: None,
      images: Vec::new(),
      response_items: Vec::new(),
      response_model: None,
    }
  }

  pub fn text(role: Role, content: impl Into<String>) -> Self {
    Self {
      content: Some(content.into()),
      ..Self::empty(role)
    }
  }

  pub fn tool(id: impl Into<String>, content: impl Into<String>) -> Self {
    Self {
      tool_call_id: Some(id.into()),
      ..Self::text(Role::Tool, content)
    }
  }

  pub fn user(content: impl Into<String>, images: Vec<Image>) -> Self {
    Self {
      images,
      ..Self::text(Role::User, content)
    }
  }

  pub fn assistant(
    content: Option<String>,
    thinking: Option<Thinking>,
    tool_calls: Vec<ToolCall>,
  ) -> Self {
    Self {
      content,
      thinking,
      tool_calls,
      ..Self::empty(Role::Assistant)
    }
  }
}

/// The key a provider files a call's raw arguments under when they were not valid JSON. The
/// call is kept rather than failing the reply, so the model can be told and try again.
pub const INVALID_ARGUMENTS: &str = "_invalid_arguments";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
  pub id: String,
  pub name: String,
  pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
  pub name: String,
  pub description: String,
  pub parameters: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
  pub input_tokens: u64,
  pub output_tokens: u64,
  // only a process provider that reports its own spend (Claude Code's stream-json result)
  // ever sets this; nobody here prices tokens, so an http provider leaves it None
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cost_usd: Option<f64>,
}
