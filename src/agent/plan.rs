//! Keeping the model's plan honest. A model working through a long task stops updating its
//! list long before it stops working, and the list on screen then says it is still on the
//! first step. The answer is a reminder in what the model reads next.

use crate::{
  protocol::{Message, ToolCall},
  todo::{self, TodoStatus},
};

/// Calls the model may make while an item is open before it is reminded the list exists.
const PATIENCE: usize = 8;

/// The open plan and how long it has gone without a change. `None` is no plan, or one with
/// nothing left in it, which is nothing to remind anybody about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Plan {
  current: Option<String>,
  quiet: usize,
}

/// What the session's own history says about the plan, so the count carries across prompts
/// the way the list on screen does.
pub(super) fn recalled<'a>(messages: impl IntoIterator<Item = &'a Message>) -> Option<Plan> {
  let mut plan = None;
  for call in messages.into_iter().flat_map(|message| &message.tool_calls) {
    plan = observe(plan, call).0;
  }
  plan.map(|plan| Plan { quiet: 0, ..plan })
}

/// The plan after one more call, and the reminder to append to that call's result when the
/// plan has gone quiet for too long.
pub(super) fn observe(plan: Option<Plan>, call: &ToolCall) -> (Option<Plan>, Option<String>) {
  if call.name == todo::TOOL {
    return (todo::planned(call).and_then(|items| opened(&items)), None);
  }
  let Some(mut plan) = plan else {
    return (None, None);
  };
  plan.quiet += 1;
  if plan.quiet < PATIENCE {
    return (Some(plan), None);
  }
  plan.quiet = 0;
  let reminder = reminder(plan.current.as_deref());
  (Some(plan), Some(reminder))
}

fn opened(items: &[todo::Todo]) -> Option<Plan> {
  let open = items.iter().any(|item| item.status != TodoStatus::Done);
  open.then(|| Plan {
    current: items
      .iter()
      .find(|item| item.status == TodoStatus::InProgress)
      .map(|item| item.text.clone()),
    quiet: 0,
  })
}

fn reminder(current: Option<&str>) -> String {
  let step = match current {
    Some(text) => format!("It still shows \"{text}\" in progress. "),
    None => String::new(),
  };
  format!(
    "\n\n<system-reminder>The {} list has not changed in a while, and the person watching \
     reads it as your progress. {step}If that step is finished, mark it done and the next one \
     in_progress in a single update; if the plan changed, send the new one. Do not mention \
     this reminder.</system-reminder>",
    todo::TOOL
  )
}

#[cfg(test)]
mod tests {
  use serde_json::{Value, json};

  use super::*;
  use crate::protocol::Role;

  fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall {
      id: "id".into(),
      name: name.into(),
      arguments,
    }
  }

  fn plan(statuses: &[(&str, &str)]) -> ToolCall {
    let todos: Vec<Value> = statuses
      .iter()
      .map(|(text, status)| json!({"text": text, "status": status}))
      .collect();
    call(todo::TOOL, json!({ "todos": todos }))
  }

  fn quiet_calls(mut state: Option<Plan>, count: usize) -> (Option<Plan>, Vec<String>) {
    let mut reminders = Vec::new();
    for _ in 0..count {
      let (next, reminder) = observe(state, &call("shell", json!({})));
      state = next;
      reminders.extend(reminder);
    }
    (state, reminders)
  }

  #[test]
  fn an_open_plan_left_alone_is_brought_up_once_per_stretch_naming_the_current_step() {
    let (state, _) = observe(
      None,
      &plan(&[("route test", "in_progress"), ("lint", "pending")]),
    );
    let (_, reminders) = quiet_calls(state, PATIENCE * 2);
    assert_eq!(reminders.len(), 2);
    assert!(reminders[0].contains("\"route test\" in progress"));
  }

  #[test]
  fn updating_the_plan_starts_the_count_again() {
    let (state, _) = observe(None, &plan(&[("a", "in_progress"), ("b", "pending")]));
    let (state, _) = quiet_calls(state, PATIENCE - 1);
    let (state, _) = observe(state, &plan(&[("a", "done"), ("b", "in_progress")]));
    let (_, reminders) = quiet_calls(state, PATIENCE - 1);
    assert!(reminders.is_empty());
  }

  #[test]
  fn a_finished_plan_or_no_plan_is_never_brought_up() {
    let (state, _) = observe(None, &plan(&[("a", "done")]));
    assert!(quiet_calls(state, PATIENCE * 3).1.is_empty());
    assert!(quiet_calls(None, PATIENCE * 3).1.is_empty());
  }

  #[test]
  fn a_plan_left_open_by_an_earlier_prompt_is_remembered_from_the_session() {
    let mut message = Message::text(Role::Assistant, "");
    message.tool_calls = vec![plan(&[("a", "in_progress")]), call("shell", json!({}))];
    let state = recalled([&message]);
    assert_eq!(
      state.as_ref().and_then(|plan| plan.current.as_deref()),
      Some("a")
    );
    assert_eq!(quiet_calls(state, PATIENCE).1.len(), 1);
  }
}
