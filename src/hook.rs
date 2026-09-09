// lifecycle hooks: a session can run an external command at a few fixed points, the way an
// editor runs a linter on save. session_start and session_end bookend a run, pre_tool and
// post_tool bracket a tool call.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
  config::HookDef,
  event::{Event, EventSink},
  protocol::ToolCall,
};

mod process;
use process::run_hook;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
  SessionStart,
  PreTool,
  PostTool,
  SessionEnd,
}

impl HookEvent {
  fn key(self) -> &'static str {
    match self {
      Self::SessionStart => "session_start",
      Self::PreTool => "pre_tool",
      Self::PostTool => "post_tool",
      Self::SessionEnd => "session_end",
    }
  }
}

/// Runs the hooks a config defines. Cheap to clone (an `Arc` around the config), and cheap to
/// hold when nothing is configured — nothing here does any work until an event with a matching,
/// non-empty hook list actually fires.
#[derive(Clone, Default)]
pub struct HookRunner(Arc<BTreeMap<String, Vec<HookDef>>>);

impl HookRunner {
  pub fn new(hooks: BTreeMap<String, Vec<HookDef>>) -> Self {
    Self(Arc::new(hooks))
  }

  fn defs(&self, event: HookEvent) -> &[HookDef] {
    self.0.get(event.key()).map(Vec::as_slice).unwrap_or(&[])
  }

  pub async fn session_start(&self, workspace: &Path, session_id: Uuid, events: &EventSink) {
    self
      .run_advisory(
        HookEvent::SessionStart,
        workspace,
        session_id,
        None,
        None,
        None,
        events,
      )
      .await;
  }

  pub async fn session_end(
    &self,
    workspace: &Path,
    session_id: Uuid,
    output: Option<&str>,
    error: bool,
    events: &EventSink,
  ) {
    self
      .run_advisory(
        HookEvent::SessionEnd,
        workspace,
        session_id,
        None,
        output,
        Some(error),
        events,
      )
      .await;
  }

  pub async fn post_tool(
    &self,
    workspace: &Path,
    session_id: Uuid,
    call: &ToolCall,
    output: &str,
    error: bool,
    events: &EventSink,
  ) {
    self
      .run_advisory(
        HookEvent::PostTool,
        workspace,
        session_id,
        Some(call),
        Some(output),
        Some(error),
        events,
      )
      .await;
  }

  /// Every other event only ever reports; this one can say no. A hook that exits non-zero, times
  /// out, or never starts blocks the call, quoting whatever the hook wrote to stderr — a gate
  /// that can be silently bypassed by a broken hook is not much of a gate.
  pub async fn pre_tool(
    &self,
    workspace: &Path,
    session_id: Uuid,
    call: &ToolCall,
    events: &EventSink,
  ) -> Result<(), String> {
    for def in self.defs(HookEvent::PreTool) {
      if !matches_tool(def.matcher.as_deref(), &call.name) {
        continue;
      }
      let payload = payload(
        HookEvent::PreTool,
        workspace,
        session_id,
        Some(call),
        None,
        None,
      );
      let reason = match run_hook(def, &payload, workspace).await {
        Ok(outcome) if outcome.success => continue,
        Ok(outcome) if outcome.stderr.trim().is_empty() => {
          "(hook wrote nothing to stderr)".to_string()
        }
        Ok(outcome) => outcome.stderr.trim().to_string(),
        Err(error) => format!("{error:#}"),
      };
      let message = format!("blocked by pre_tool hook {}: {reason}", describe(def));
      // the tool call itself already carries this back to the model as its error output; the
      // event is so the person watching sees it too, not buried in a tool result
      events.emit(Event::Error {
        message: message.clone(),
      });
      return Err(message);
    }
    Ok(())
  }

  #[allow(clippy::too_many_arguments)]
  async fn run_advisory(
    &self,
    event: HookEvent,
    workspace: &Path,
    session_id: Uuid,
    tool: Option<&ToolCall>,
    output: Option<&str>,
    error: Option<bool>,
    events: &EventSink,
  ) {
    for def in self.defs(event) {
      if let Some(call) = tool
        && !matches_tool(def.matcher.as_deref(), &call.name)
      {
        continue;
      }
      let payload = payload(event, workspace, session_id, tool, output, error);
      let outcome = match run_hook(def, &payload, workspace).await {
        Ok(outcome) if outcome.success => continue,
        Ok(outcome) => outcome.stderr,
        Err(error) => format!("{error:#}"),
      };
      events.emit(Event::Error {
        message: format!(
          "{} hook {} failed: {}",
          event.key(),
          describe(def),
          if outcome.trim().is_empty() {
            "(no output)"
          } else {
            outcome.trim()
          }
        ),
      });
    }
  }
}

// no matcher always matches; a tool event with one treats it as a plain substring unless it
// carries a `*`, in which case that one wildcard is honored anywhere in the pattern
fn matches_tool(matcher: Option<&str>, name: &str) -> bool {
  match matcher {
    None => true,
    Some(pattern) if pattern.contains('*') => glob_match(pattern, name),
    Some(pattern) => name.contains(pattern),
  }
}

fn glob_match(pattern: &str, text: &str) -> bool {
  let segments: Vec<&str> = pattern.split('*').collect();
  let mut rest = text;
  for (index, segment) in segments.iter().enumerate() {
    if segment.is_empty() {
      continue;
    }
    if index == segments.len() - 1 {
      return rest.ends_with(segment);
    }
    match rest.find(segment) {
      Some(position) if index == 0 && position != 0 => return false,
      Some(position) => rest = &rest[position + segment.len()..],
      None => return false,
    }
  }
  true
}

fn describe(def: &HookDef) -> String {
  def.command.join(" ")
}

fn payload(
  event: HookEvent,
  workspace: &Path,
  session_id: Uuid,
  tool: Option<&ToolCall>,
  output: Option<&str>,
  error: Option<bool>,
) -> Value {
  let mut value = json!({
    "event": event,
    "workspace": workspace.display().to_string(),
    "session_id": session_id.to_string(),
  });
  let map = value.as_object_mut().expect("payload is always an object");
  if let Some(call) = tool {
    map.insert(
      "tool".into(),
      json!({"name": call.name, "arguments": call.arguments}),
    );
  }
  if let Some(output) = output {
    map.insert("output".into(), json!(output));
  }
  if let Some(error) = error {
    map.insert("error".into(), json!(error));
  }
  value
}
