use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use crate::{config::PermissionRules, protocol::ToolCall};

pub(crate) async fn subject(workspace: &Path, call: &ToolCall) -> Result<Option<String>> {
  if matches!(call.name.as_str(), "read" | "write" | "edit" | "list") {
    let input = call
      .arguments
      .get("path")
      .and_then(Value::as_str)
      .unwrap_or(".");
    let root = tokio::fs::canonicalize(workspace).await?;
    let path = crate::workspace::writable(&root, input).await?;
    return Ok(Some(
      path.strip_prefix(root)?.to_string_lossy().into_owned(),
    ));
  }
  Ok(crate::agent::subject(&call.arguments).map(str::to_owned))
}

pub(crate) async fn normalize(
  rules: &PermissionRules,
  workspace: &std::path::Path,
  name: &str,
) -> Result<PermissionRules> {
  if !matches!(name, "read" | "write" | "edit" | "list") {
    return Ok(rules.clone());
  }
  let root = tokio::fs::canonicalize(workspace).await?;
  let mut normalized = rules.clone();
  for rule in normalized
    .allow
    .iter_mut()
    .chain(normalized.deny.iter_mut())
  {
    let Some((tool, rest)) = rule.split_once('(') else {
      continue;
    };
    if tool.trim() != name {
      continue;
    }
    let input = rest.trim_end_matches(')');
    let wildcard = input.ends_with('*');
    let input = input.strip_suffix('*').unwrap_or(input);
    let path = crate::workspace::writable(&root, input).await?;
    let mut subject = path.strip_prefix(&root)?.to_string_lossy().into_owned();
    if input.ends_with('/') && !subject.is_empty() {
      subject.push('/');
    }
    *rule = format!("{name}({subject}{})", if wildcard { "*" } else { "" });
  }
  Ok(normalized)
}
