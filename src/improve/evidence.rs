use std::path::Path;

// dependencies
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// project
use crate::{SessionStore, protocol::Role};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Evidence {
  pub sessions: Vec<Uuid>,
  pub text: String,
}

/// Bounded excerpts from the active branches of recent runs in one workspace.
pub async fn evidence(store: &SessionStore, workspace: &Path, limit: usize) -> Result<Evidence> {
  ensure!((1..=20).contains(&limit), "session limit must be 1..=20");
  let mut evidence = Evidence {
    sessions: Vec::new(),
    text: String::new(),
  };
  for info in store
    .list()
    .await?
    .into_iter()
    .filter(|info| info.workspace == workspace)
    .take(limit)
  {
    let session = store.load(info.id).await?;
    let messages = session.messages()?;
    evidence.sessions.push(info.id);
    evidence.text.push_str(&format!("\nSession {}\n", info.id));
    let start = messages.len().saturating_sub(16);
    for (index, message) in messages.iter().enumerate() {
      if (index < start && index != 0) || message.role == Role::System {
        continue;
      }
      if let Some(content) = &message.content {
        evidence.text.push_str(&format!(
          "{:?} {}: {}\n",
          message.role,
          message.tool_call_id.as_deref().unwrap_or(""),
          excerpt(content)
        ));
      }
      for call in message.tool_calls.iter().take(8) {
        let subject: String = crate::agent::subject(&call.arguments)
          .unwrap_or("")
          .chars()
          .take(240)
          .collect();
        evidence.text.push_str(&format!(
          "tool requested {}: {} {subject}\n",
          call.id, call.name
        ));
      }
      if evidence.text.len() >= 48 * 1024 {
        evidence.text = crate::tool::truncate(evidence.text, 48 * 1024);
        return Ok(evidence);
      }
    }
  }
  Ok(evidence)
}

fn excerpt(text: &str) -> String {
  if text.chars().take(1201).count() <= 1200 {
    return text.into();
  }
  let head: String = text.chars().take(600).collect();
  // status and the last error often follow a long command's output
  let tail: Vec<_> = text.chars().rev().take(600).collect();
  let tail: String = tail.into_iter().rev().collect();
  format!("{head}\n[excerpt omitted]\n{tail}")
}
