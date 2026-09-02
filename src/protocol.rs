use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
  System,
  User,
  Assistant,
  Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Message {
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
pub struct Image {
  pub url: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

impl Image {
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
    let data = tokio::fs::read(path)
      .await
      .with_context(|| format!("read image {}", path.display()))?;
    if data.len() > 20 * 1024 * 1024 {
      bail!("image exceeds the 20 MiB limit: {}", path.display());
    }
    Ok(Self {
      url: format!("data:{media_type};base64,{}", STANDARD.encode(data)),
      detail: None,
    })
  }
}

impl Message {
  pub fn text(role: Role, content: impl Into<String>) -> Self {
    Self {
      role,
      content: Some(content.into()),
      tool_calls: Vec::new(),
      tool_call_id: None,
      images: Vec::new(),
    }
  }

  pub fn tool(id: impl Into<String>, content: impl Into<String>) -> Self {
    Self {
      role: Role::Tool,
      content: Some(content.into()),
      tool_calls: Vec::new(),
      tool_call_id: Some(id.into()),
      images: Vec::new(),
    }
  }

  pub fn user(content: impl Into<String>, images: Vec<Image>) -> Self {
    Self {
      role: Role::User,
      content: Some(content.into()),
      tool_calls: Vec::new(),
      tool_call_id: None,
      images,
    }
  }
}

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
}
