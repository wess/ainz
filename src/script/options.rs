//! `ainz.opt` writes applied to the configuration. Each write is a path and a value, applied to
//! the configuration as it stands when it is applied, so a write from a keymap never undoes a
//! change made in /settings a moment before.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::Config;

/// The configuration with the writes applied, and a sentence for each write that could not be.
pub fn apply(config: &Config, writes: Vec<(Vec<Value>, Value)>) -> (Config, Vec<String>) {
  let mut current = config.clone();
  let mut problems = Vec::new();
  for (path, value) in writes {
    match write(&current, &path, value) {
      Ok(next) => current = next,
      Err(error) => problems.push(format!("ainz.opt.{}: {error:#}", name(&path))),
    }
  }
  (current, problems)
}

fn write(config: &Config, path: &[Value], value: Value) -> Result<Config> {
  let mut tree = serde_json::to_value(config)?;
  set(&mut tree, path, value.clone())?;
  let next: Config = serde_json::from_value(tree)
    .map_err(|error| anyhow::anyhow!("{}", error.to_string().replace("invalid type: ", "")))?;
  // a name the configuration does not have is dropped on the way back in; say so rather than
  // letting a typo look like it worked
  if !value.is_null() {
    let back = serde_json::to_value(&next)?;
    if lookup(&back, path).is_none() {
      bail!("no such option");
    }
  }
  next
    .validate_options()
    .context("the configuration would not be valid")?;
  Ok(next)
}

fn set(tree: &mut Value, path: &[Value], value: Value) -> Result<()> {
  let Some((last, parents)) = path.split_last() else {
    bail!("an option needs a name");
  };
  let mut node = tree;
  for key in parents {
    node = child(node, key, true)?;
  }
  match (node, last) {
    (Value::Object(map), Value::String(key)) => {
      // a Lua table that was empty cannot say whether it was a list
      let value = match (map.get(key), value) {
        (Some(Value::Array(_)), Value::Object(empty)) if empty.is_empty() => {
          Value::Array(Vec::new())
        }
        (_, value) => value,
      };
      if value.is_null() {
        map.remove(key);
      } else {
        map.insert(key.clone(), value);
      }
    }
    (Value::Array(items), Value::Number(index)) => {
      let index = position(index)?;
      match index.cmp(&items.len()) {
        std::cmp::Ordering::Less if value.is_null() => {
          items.remove(index);
        }
        std::cmp::Ordering::Less => items[index] = value,
        std::cmp::Ordering::Equal if !value.is_null() => items.push(value),
        _ => bail!("index {} is past the end of the list", index + 1),
      }
    }
    _ => bail!("cannot be set this way"),
  }
  Ok(())
}

fn child<'a>(node: &'a mut Value, key: &Value, create: bool) -> Result<&'a mut Value> {
  match (node, key) {
    (Value::Object(map), Value::String(key)) => {
      if create && !map.contains_key(key) {
        map.insert(key.clone(), Value::Object(Default::default()));
      }
      map.get_mut(key).context("no such option")
    }
    (Value::Array(items), Value::Number(index)) => {
      let index = position(index)?;
      items.get_mut(index).context("no such entry")
    }
    _ => bail!("no such option"),
  }
}

fn lookup<'a>(tree: &'a Value, path: &[Value]) -> Option<&'a Value> {
  path.iter().try_fold(tree, |node, key| match (node, key) {
    (Value::Object(map), Value::String(key)) => map.get(key),
    (Value::Array(items), Value::Number(index)) => items.get(position(index).ok()?),
    _ => None,
  })
}

/// Lua counts from one.
fn position(index: &serde_json::Number) -> Result<usize> {
  index
    .as_u64()
    .filter(|index| *index >= 1)
    .map(|index| index as usize - 1)
    .context("a list index counts from 1")
}

fn name(path: &[Value]) -> String {
  path
    .iter()
    .map(|key| match key {
      Value::String(key) => key.clone(),
      other => format!("[{other}]"),
    })
    .collect::<Vec<_>>()
    .join(".")
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn path(keys: &[&str]) -> Vec<Value> {
    keys.iter().map(|key| json!(key)).collect()
  }

  #[test]
  fn a_write_sets_a_nested_option() {
    let (config, problems) = apply(
      &Config::default(),
      vec![
        (path(&["ui", "vim"]), json!(true)),
        (path(&["max_steps"]), json!(64)),
      ],
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(config.ui.vim);
    assert_eq!(config.max_steps, 64);
  }

  #[test]
  fn a_misspelt_option_or_a_wrong_type_is_reported_and_skipped() {
    let (config, problems) = apply(
      &Config::default(),
      vec![
        (path(&["ui", "vmi"]), json!(true)),
        (path(&["max_steps"]), json!("lots")),
        (path(&["max_steps"]), json!(12)),
      ],
    );
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].contains("ui.vmi"));
    assert_eq!(config.max_steps, 12);
  }

  #[test]
  fn an_invalid_configuration_is_refused() {
    let (config, problems) = apply(&Config::default(), vec![(path(&["max_steps"]), json!(0))]);
    assert_eq!(problems.len(), 1);
    assert_eq!(config.max_steps, Config::default().max_steps);
  }

  #[test]
  fn a_list_grows_by_writing_one_past_its_end_and_an_empty_table_is_a_list() {
    let mut first = path(&["fallback_providers"]);
    let (config, problems) = apply(&Config::default(), vec![(first.clone(), json!({}))]);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(config.fallback_providers.is_empty());
    first.push(json!(1));
    let (config, problems) = apply(&config, vec![(first, json!("backup"))]);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(config.fallback_providers, ["backup"]);
  }

  #[test]
  fn nil_clears_an_optional_value() {
    let config = Config {
      budget_usd: Some(2.0),
      ..Config::default()
    };
    let (config, problems) = apply(&config, vec![(path(&["budget_usd"]), Value::Null)]);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(config.budget_usd, None);
  }
}
