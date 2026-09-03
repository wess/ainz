use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoState {
  Pending,
  Doing,
  Done,
}

impl TodoState {
  fn marker(self) -> &'static str {
    match self {
      Self::Pending => "[ ]",
      Self::Doing => "[>]",
      Self::Done => "[x]",
    }
  }
}

#[derive(Clone, Debug)]
pub struct TodoItem {
  pub text: String,
  pub state: TodoState,
}

/// A session's plan: short steps with a state, kept in memory only. Nothing here survives the
/// process, and nothing should — it is scratch space for the current run, not a record.
#[derive(Clone, Default)]
pub struct TodoList {
  items: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoList {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn set(&self, texts: Vec<String>) {
    let items = texts
      .into_iter()
      .map(|text| TodoItem {
        text,
        state: TodoState::Pending,
      })
      .collect();
    *self.items.lock().unwrap() = items;
  }

  // a step is named by its 1-based position in the rendered list, or by matching its text
  // exactly — either is unambiguous once the model has seen the list at all
  fn locate(items: &[TodoItem], target: &str) -> Result<usize> {
    let target = target.trim();
    if let Ok(index) = target.parse::<usize>() {
      return if index >= 1 && index <= items.len() {
        Ok(index - 1)
      } else {
        bail!(
          "no item at index {index}; the list has {} item(s)",
          items.len()
        )
      };
    }
    items
      .iter()
      .position(|item| item.text == target)
      .with_context(|| format!("no item matches {target:?}"))
  }

  pub fn start(&self, target: &str) -> Result<()> {
    let mut items = self.items.lock().unwrap();
    let index = Self::locate(&items, target)?;
    // only one step is ever doing: a plan with two things in flight at once isn't telling
    // the model what to do next, so starting one always un-starts whatever was doing before
    for item in items.iter_mut() {
      if item.state == TodoState::Doing {
        item.state = TodoState::Pending;
      }
    }
    items[index].state = TodoState::Doing;
    Ok(())
  }

  pub fn done(&self, target: &str) -> Result<()> {
    let mut items = self.items.lock().unwrap();
    let index = Self::locate(&items, target)?;
    items[index].state = TodoState::Done;
    Ok(())
  }

  pub fn render(&self) -> String {
    let items = self.items.lock().unwrap();
    if items.is_empty() {
      return "(no plan set)".to_string();
    }
    items
      .iter()
      .enumerate()
      .map(|(index, item)| format!("{}. {} {}", index + 1, item.state.marker(), item.text))
      .collect::<Vec<_>>()
      .join("\n")
  }

  pub fn tool(&self) -> Arc<dyn Tool> {
    Arc::new(TodoTool { list: self.clone() })
  }
}

struct TodoTool {
  list: TodoList,
}

#[derive(Deserialize)]
struct TodoArgs {
  action: String,
  #[serde(default)]
  items: Vec<String>,
  target: Option<String>,
}

#[async_trait]
impl Tool for TodoTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "todo".into(),
      description: "Keep a short plan for this session, in memory only. `set` replaces the \
        whole list with the given step texts, `start` marks one step doing (by its number in \
        the list or its exact text; this un-starts whatever was doing), `done` marks one step \
        done, `list` just shows it. Every action returns the whole list so the plan stays in \
        view."
        .into(),
      parameters: json!({
        "type": "object", "properties": {
          "action": {"type": "string", "enum": ["set", "start", "done", "list"]},
          "items": {"type": "array", "items": {"type": "string"}},
          "target": {"type": "string"}
        }, "required": ["action"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    // it changes state kept for this session only, nothing outside it
    Risk::Read
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let args: TodoArgs = serde_json::from_value(arguments)?;
    match args.action.as_str() {
      "set" => self.list.set(args.items),
      "start" => {
        let target = args.target.context("target is required to start an item")?;
        self.list.start(&target)?;
      }
      "done" => {
        let target = args
          .target
          .context("target is required to mark an item done")?;
        self.list.done(&target)?;
      }
      "list" => {}
      other => bail!("unknown todo action: {other}"),
    }
    Ok(truncate(self.list.render(), context.max_output_bytes))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn starting_an_item_unstarts_the_previous_one() {
    let list = TodoList::new();
    list.set(vec!["a".into(), "b".into()]);
    list.start("1").unwrap();
    list.start("2").unwrap();
    let rendered = list.render();
    assert!(rendered.contains("[ ] a"), "{rendered}");
    assert!(rendered.contains("[>] b"), "{rendered}");
  }

  #[test]
  fn an_out_of_range_index_is_a_clear_error() {
    let list = TodoList::new();
    list.set(vec!["only one".into()]);
    let error = list.start("2").unwrap_err().to_string();
    assert!(error.contains("2"), "{error}");
    assert!(error.contains("1 item"), "{error}");
  }
}
