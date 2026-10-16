use std::sync::Arc;

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
  event::Event,
  protocol::{ToolCall, ToolSpec},
  tool::{Risk, Tool, ToolContext},
};

/// The tool whose calls are the plan.
pub const TOOL: &str = "todo";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
  #[default]
  Pending,
  InProgress,
  Done,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Todo {
  pub text: String,
  #[serde(default)]
  pub status: TodoStatus,
}

#[derive(Deserialize)]
struct TodoArgs {
  todos: Vec<Todo>,
}

/// The plan a call sets, when it is a todo call whose list is well formed. The plan is never
/// held anywhere but in the calls themselves, so whoever needs it, a resumed transcript or a
/// reminder, reads the last one.
pub fn planned(call: &ToolCall) -> Option<Vec<Todo>> {
  if call.name != TOOL {
    return None;
  }
  let args = TodoArgs::deserialize(&call.arguments).ok()?;
  check(&args.todos).ok()?;
  Some(args.todos)
}

fn check(todos: &[Todo]) -> Result<()> {
  if todos.iter().any(|todo| todo.text.trim().is_empty()) {
    bail!("every item needs text");
  }
  // a plan with two things in flight is not telling anybody what happens next
  let doing = todos
    .iter()
    .filter(|todo| todo.status == TodoStatus::InProgress)
    .count();
  if doing > 1 {
    bail!("{doing} items are in_progress; mark exactly one while you work on it");
  }
  Ok(())
}

/// The plan for a session. It keeps no state of its own: every call carries the whole list,
/// so a subagent's plan can't land in its parent's and a rebuilt agent can't lose one.
pub fn tool() -> Arc<dyn Tool> {
  Arc::new(TodoTool)
}

struct TodoTool;

#[async_trait]
impl Tool for TodoTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: TOOL.into(),
      description: "Keep the plan for a multi-step task. Send the whole list every time; it \
                    replaces the previous one. Mark exactly one item in_progress while you \
                    work on it, and mark it done and the next one in_progress in the same \
                    update. Skip this for work that is one or two steps."
        .into(),
      parameters: json!({
        "type": "object", "properties": {
          "todos": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "text": {"type": "string", "minLength": 1},
                "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
              },
              "required": ["text", "status"],
              "additionalProperties": false
            }
          }
        }, "required": ["todos"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Read
  }

  fn validate(&self, arguments: &Value) -> Result<()> {
    check(&TodoArgs::deserialize(arguments)?.todos)
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let TodoArgs { todos } = serde_json::from_value(arguments)?;
    check(&todos)?;
    let done = todos
      .iter()
      .filter(|todo| todo.status == TodoStatus::Done)
      .count();
    let total = todos.len();
    if let Some((events, _)) = &context.progress {
      events.emit(Event::Todos { items: todos });
    }
    Ok(format!("{done}/{total} done"))
  }
}
