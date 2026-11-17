//! Saving only what changed. init.lua and command-line flags change the configuration for a
//! session without anybody asking for config.toml to change; a save made from /settings later
//! writes the settings that were changed, and leaves everything else as the file had it.

use serde_json::Value;

/// Applies to `target` every change between `before` and `after`. A value that did not change
/// keeps whatever `target` has for it; one that changed, or appeared, takes the new value; one
/// that went away goes away.
pub(crate) fn carry(target: &mut Value, before: &Value, after: &Value) {
  match (before, after) {
    (Value::Object(before), Value::Object(after)) => {
      if !target.is_object() {
        *target = Value::Object(Default::default());
      }
      let Value::Object(target) = target else {
        return;
      };
      for (key, next) in after {
        match before.get(key) {
          Some(previous) if previous == next => {}
          Some(previous) => carry(
            target.entry(key.clone()).or_insert(Value::Null),
            previous,
            next,
          ),
          None => {
            target.insert(key.clone(), next.clone());
          }
        }
      }
      for key in before.keys() {
        if !after.contains_key(key) {
          target.remove(key);
        }
      }
    }
    _ if before != after => *target = after.clone(),
    _ => {}
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn only_what_changed_reaches_the_target() {
    let mut file = json!({"model": "from-file", "ui": {"vim": false, "bell": true}});
    let session = json!({"model": "from-init-lua", "ui": {"vim": false, "bell": true}});
    let edited = json!({"model": "from-init-lua", "ui": {"vim": true, "bell": true}});
    carry(&mut file, &session, &edited);
    assert_eq!(
      file,
      json!({"model": "from-file", "ui": {"vim": true, "bell": true}})
    );
  }

  #[test]
  fn a_new_value_is_added_and_a_removed_one_is_removed() {
    let mut file = json!({"budget_usd": 3.0});
    carry(
      &mut file,
      &json!({"budget_usd": 3.0}),
      &json!({"max_steps": 9}),
    );
    assert_eq!(file, json!({"max_steps": 9}));
  }
}
