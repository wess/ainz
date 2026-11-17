//! The `ainz` table a script sees. Everything here records what was asked for, in the registry
//! or as an effect; nothing reaches into the interface or the agent directly.

use std::{
  sync::{Arc, PoisonError, atomic::Ordering},
  time::{Duration, Instant},
};

use mlua::{Function, Lua, LuaSerdeExt, Table, Value};
use serde_json::Value as Json;

use super::{
  Effect, Level, Panel, Side,
  registry::{Command, Keymap, Registry, Rhs, Shared, Timer, ToolDef},
  text::{self, Line, Segment, Style},
};
use crate::tool::Risk;

/// The longest a panel's history gets when lines are appended to it.
const PANEL_LINES: usize = 1000;

pub(super) fn install(
  lua: &Lua,
  shared: Arc<Shared>,
  workspace: &std::path::Path,
) -> mlua::Result<()> {
  let ainz = lua.create_table()?;
  ainz.set("version", env!("CARGO_PKG_VERSION"))?;
  ainz.set("workspace", workspace.display().to_string())?;
  ainz.set("__opt", lua.create_table()?)?;
  let actions = lua.create_table()?;
  for action in crate::keybind::Action::ALL {
    if let Ok(Json::String(name)) = serde_json::to_value(action) {
      actions.set(name, true)?;
    }
  }
  ainz.set("__actions", actions)?;

  let state = shared.clone();
  ainz.set(
    "__write",
    lua.create_function(move |lua, (path, value): (Vec<Value>, Value)| {
      let path = path
        .iter()
        .map(|key| json(lua, key))
        .collect::<mlua::Result<Vec<_>>>()?;
      let value = json(lua, &value)?;
      state
        .writes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((path, value));
      Ok(())
    })?,
  )?;

  let state = shared.clone();
  ainz.set(
    "notify",
    lua.create_function(move |_, (text, level): (String, Option<String>)| {
      let level = match level.as_deref() {
        Some("warn" | "warning") => Level::Warn,
        Some("error") => Level::Error,
        _ => Level::Info,
      };
      effect(&state, Effect::Notify { text, level });
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "send",
    lua.create_function(move |_, text: String| {
      effect(&state, Effect::Send(text));
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "cmd",
    lua.create_function(move |_, text: String| {
      let text = match text.starts_with('/') {
        true => text,
        false => format!("/{text}"),
      };
      effect(&state, Effect::Send(text));
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "quit",
    lua.create_function(move |_, ()| {
      effect(&state, Effect::Quit);
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "state",
    lua.create_function(move |lua, ()| {
      let view = state
        .view
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
      lua.to_value_with(&view, super::serialize())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "redraw",
    lua.create_function(move |_, ()| {
      redraw(&state);
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "highlight",
    lua.create_function(move |_, (role, value): (String, String)| {
      let color = text::color(&value)
        .ok_or_else(|| mlua::Error::runtime(format!("{value} is not a colour")))?;
      effect(&state, Effect::Highlight { role, color });
      Ok(())
    })?,
  )?;

  ainz.set("input", input(lua, &shared)?)?;
  ainz.set("keymap", keymap(lua, &shared)?)?;
  ainz.set("statusbar", statusbar(lua, &shared)?)?;
  ainz.set("topic", topic(lua, &shared)?)?;
  ainz.set("json", json_table(lua)?)?;
  events(lua, &ainz, &shared)?;
  timers(lua, &ainz, &shared)?;
  panels(lua, &ainz, &shared)?;
  tools(lua, &ainz, &shared)?;
  lua.globals().set("ainz", ainz)
}

fn effect(shared: &Shared, effect: Effect) {
  shared
    .effects
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
    .push(effect);
}

fn redraw(shared: &Shared) {
  shared.generation.fetch_add(1, Ordering::Relaxed);
  shared.redraw.store(true, Ordering::Relaxed);
}

fn registry(shared: &Shared) -> std::sync::MutexGuard<'_, Registry> {
  shared
    .registry
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
}

fn json(lua: &Lua, value: &Value) -> mlua::Result<Json> {
  lua.from_value_with(
    value.clone(),
    mlua::serde::DeserializeOptions::new().deny_unsupported_types(false),
  )
}

fn input(lua: &Lua, shared: &Arc<Shared>) -> mlua::Result<Table> {
  let table = lua.create_table()?;
  let state = shared.clone();
  table.set(
    "get",
    lua.create_function(move |_, ()| {
      Ok(
        state
          .view
          .lock()
          .unwrap_or_else(PoisonError::into_inner)
          .input
          .clone(),
      )
    })?,
  )?;
  let state = shared.clone();
  table.set(
    "set",
    lua.create_function(move |_, text: String| {
      effect(&state, Effect::SetInput(text));
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  table.set(
    "insert",
    lua.create_function(move |_, text: String| {
      effect(&state, Effect::Insert(text));
      Ok(())
    })?,
  )?;
  Ok(table)
}

fn keymap(lua: &Lua, shared: &Arc<Shared>) -> mlua::Result<Table> {
  let table = lua.create_table()?;
  let state = shared.clone();
  table.set(
    "__set",
    lua.create_function(
      move |_, (keys, rhs, opts): (String, Value, Option<Table>)| {
        let rhs = match rhs {
          Value::Function(function) => Rhs::Function(function),
          Value::String(text) => Rhs::Text(text.to_str()?.to_string()),
          _ => {
            return Err(mlua::Error::runtime(
              "a keymap runs a function or sends text",
            ));
          }
        };
        let desc = opts
          .and_then(|opts| opts.get::<Option<String>>("desc").ok().flatten())
          .unwrap_or_default();
        let keys = keys.trim().to_ascii_lowercase();
        let mut registry = registry(&state);
        registry.keymaps.retain(|keymap| keymap.keys != keys);
        let id = registry.id();
        registry.keymaps.push(Keymap {
          id,
          keys,
          rhs,
          desc,
        });
        Ok(())
      },
    )?,
  )?;
  let state = shared.clone();
  table.set(
    "del",
    lua.create_function(move |_, keys: String| {
      let keys = keys.trim().to_ascii_lowercase();
      registry(&state)
        .keymaps
        .retain(|keymap| keymap.keys != keys);
      Ok(())
    })?,
  )?;
  Ok(table)
}

fn events(lua: &Lua, ainz: &Table, shared: &Arc<Shared>) -> mlua::Result<()> {
  let state = shared.clone();
  ainz.set(
    "on",
    lua.create_function(move |_, (event, handler): (String, Function)| {
      let mut registry = registry(&state);
      let id = registry.id();
      registry
        .handlers
        .entry(event)
        .or_default()
        .push((id, handler));
      Ok(id)
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "off",
    lua.create_function(move |_, id: u64| {
      for handlers in registry(&state).handlers.values_mut() {
        handlers.retain(|(handler, _)| *handler != id);
      }
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "command",
    lua.create_function(
      move |_, (name, function, opts): (String, Function, Option<Table>)| {
        let name = name.trim().trim_start_matches('/').to_string();
        if name.is_empty()
          || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
          return Err(mlua::Error::runtime(format!(
            "{name:?} is not a command name"
          )));
        }
        let desc = opts
          .and_then(|opts| opts.get::<Option<String>>("desc").ok().flatten())
          .unwrap_or_default();
        registry(&state)
          .commands
          .insert(name, Command { function, desc });
        Ok(())
      },
    )?,
  )?;
  Ok(())
}

fn timers(lua: &Lua, ainz: &Table, shared: &Arc<Shared>) -> mlua::Result<()> {
  let state = shared.clone();
  ainz.set(
    "__timer",
    lua.create_function(move |_, (ms, function, repeat): (u64, Function, bool)| {
      // a repeating timer faster than this would keep the terminal awake for nothing
      let every = Duration::from_millis(ms.max(if repeat { 100 } else { 0 }));
      let mut registry = registry(&state);
      let id = registry.id();
      registry.timers.insert(
        id,
        Timer {
          every,
          due: Instant::now() + every,
          repeat,
          function,
        },
      );
      Ok(id)
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "__timer_stop",
    lua.create_function(move |_, id: u64| {
      registry(&state).timers.remove(&id);
      Ok(())
    })?,
  )?;
  Ok(())
}

fn panels(lua: &Lua, ainz: &Table, shared: &Arc<Shared>) -> mlua::Result<()> {
  let state = shared.clone();
  ainz.set(
    "__panel_open",
    lua.create_function(move |_, opts: Table| {
      let side = match opts.get::<Option<String>>("side")?.as_deref() {
        None | Some("right") => Side::Right,
        Some("left") => Side::Left,
        Some(other) => {
          return Err(mlua::Error::runtime(format!(
            "side is left or right, not {other}"
          )));
        }
      };
      let width = opts.get::<Option<u16>>("width")?.unwrap_or(28).clamp(8, 80);
      let height = opts.get::<Option<u16>>("height")?;
      let title = opts.get::<Option<String>>("title")?.unwrap_or_default();
      let follow = opts.get::<Option<bool>>("follow")?.unwrap_or(false);
      let mut registry = registry(&state);
      let id = registry.id();
      registry.panels.insert(
        id,
        Panel {
          id,
          side,
          width,
          height,
          title,
          lines: Vec::new(),
          follow,
        },
      );
      drop(registry);
      redraw(&state);
      Ok(id)
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "__panel_set",
    lua.create_function(move |_, (id, lines): (u64, Table)| {
      let lines = lines
        .sequence_values::<Value>()
        .map(|line| line.and_then(|line| self::line(&line)))
        .collect::<mlua::Result<Vec<_>>>()?;
      edit(&state, id, |panel| panel.lines = lines)
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "__panel_append",
    lua.create_function(move |_, (id, value): (u64, Value)| {
      let line = line(&value)?;
      edit(&state, id, |panel| {
        panel.lines.push(line);
        let excess = panel.lines.len().saturating_sub(PANEL_LINES);
        panel.lines.drain(..excess);
      })
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "__panel_title",
    lua.create_function(move |_, (id, title): (u64, String)| {
      edit(&state, id, |panel| panel.title = title)
    })?,
  )?;
  let state = shared.clone();
  ainz.set(
    "__panel_close",
    lua.create_function(move |_, id: u64| {
      registry(&state).panels.remove(&id);
      redraw(&state);
      Ok(())
    })?,
  )?;
  Ok(())
}

fn edit(shared: &Shared, id: u64, change: impl FnOnce(&mut Panel)) -> mlua::Result<()> {
  let mut registry = registry(shared);
  let panel = registry
    .panels
    .get_mut(&id)
    .ok_or_else(|| mlua::Error::runtime("that panel is closed"))?;
  change(panel);
  drop(registry);
  redraw(shared);
  Ok(())
}

fn statusbar(lua: &Lua, shared: &Arc<Shared>) -> mlua::Result<Table> {
  let table = lua.create_table()?;
  let state = shared.clone();
  table.set(
    "item",
    lua.create_function(move |_, (name, function): (String, Option<Function>)| {
      let mut registry = registry(&state);
      match function {
        Some(function) => registry.items.insert(name, function),
        None => registry.items.remove(&name),
      };
      drop(registry);
      redraw(&state);
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  table.set(
    "set",
    lua.create_function(move |_, items: Option<Vec<String>>| {
      registry(&state).bar = items;
      redraw(&state);
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  table.set(
    "render",
    lua.create_function(move |_, function: Option<Function>| {
      registry(&state).statusline = function;
      redraw(&state);
      Ok(())
    })?,
  )?;
  let state = shared.clone();
  table.set(
    "refresh",
    lua.create_function(move |_, ()| {
      redraw(&state);
      Ok(())
    })?,
  )?;
  Ok(table)
}

fn topic(lua: &Lua, shared: &Arc<Shared>) -> mlua::Result<Table> {
  let table = lua.create_table()?;
  let state = shared.clone();
  table.set(
    "render",
    lua.create_function(move |_, function: Option<Function>| {
      registry(&state).topic = function;
      redraw(&state);
      Ok(())
    })?,
  )?;
  Ok(table)
}

fn tools(lua: &Lua, ainz: &Table, shared: &Arc<Shared>) -> mlua::Result<()> {
  let state = shared.clone();
  ainz.set(
    "tool",
    lua.create_function(move |lua, spec: Table| {
      let name: String = spec.get("name")?;
      if name.is_empty()
        || !name
          .chars()
          .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
      {
        return Err(mlua::Error::runtime(format!(
          "tool name {name:?} must be lowercase letters, digits, and underscores"
        )));
      }
      let description: String = spec.get("description")?;
      let parameters = match spec.get::<Value>("parameters")? {
        Value::Nil => serde_json::json!({"type": "object", "properties": {}}),
        value => json(lua, &value)?,
      };
      let risk = match spec.get::<Option<String>>("risk")?.as_deref() {
        Some("read") => Risk::Read,
        Some("write") => Risk::Write,
        None | Some("execute") => Risk::Execute,
        Some("network") => Risk::Network,
        Some(other) => {
          return Err(mlua::Error::runtime(format!(
            "risk is read, write, execute, or network, not {other}"
          )));
        }
      };
      let function: Function = spec.get("run")?;
      registry(&state).tools.insert(
        name,
        ToolDef {
          description,
          parameters,
          risk,
          function,
        },
      );
      effect(&state, Effect::Rebuild);
      Ok(())
    })?,
  )?;
  Ok(())
}

fn json_table(lua: &Lua) -> mlua::Result<Table> {
  let table = lua.create_table()?;
  table.set(
    "encode",
    lua.create_function(|lua, value: Value| {
      serde_json::to_string(&json(lua, &value)?).map_err(mlua::Error::external)
    })?,
  )?;
  table.set(
    "decode",
    lua.create_function(|lua, text: String| {
      let value: Json = serde_json::from_str(&text).map_err(mlua::Error::external)?;
      lua.to_value_with(&value, super::serialize())
    })?,
  )?;
  Ok(table)
}

/// A line as a script writes one: a string of format codes, a single segment such as
/// `{ "text", fg = "red", bold = true }`, or a list of strings and segments.
pub(super) fn line(value: &Value) -> mlua::Result<Line> {
  match value {
    Value::Nil => Ok(Line::new()),
    Value::String(text) => Ok(text::parse(&text.to_string_lossy())),
    Value::Number(_) | Value::Integer(_) | Value::Boolean(_) => {
      Ok(text::parse(&value.to_string()?))
    }
    Value::Table(table) => {
      // a table that styles its text is one segment; any other is a list of parts
      for key in ["text", "fg", "bg", "bold", "italic", "underline", "reverse"] {
        if !table.get::<Value>(key)?.is_nil() {
          return Ok(vec![segment(table)?]);
        }
      }
      let mut line = Line::new();
      for part in table.sequence_values::<Value>() {
        line.extend(self::line(&part?)?);
      }
      Ok(line)
    }
    _ => Err(mlua::Error::runtime(
      "a line is a string or a table of segments",
    )),
  }
}

fn segment(table: &Table) -> mlua::Result<Segment> {
  let text = match table.get::<Option<String>>(1)? {
    Some(text) => text,
    None => table.get::<Option<String>>("text")?.unwrap_or_default(),
  };
  let color = |key: &str| -> mlua::Result<_> {
    Ok(
      table
        .get::<Option<String>>(key)?
        .as_deref()
        .and_then(text::color),
    )
  };
  let flag =
    |key: &str| -> mlua::Result<bool> { Ok(table.get::<Option<bool>>(key)?.unwrap_or(false)) };
  Ok(Segment {
    text,
    style: Style {
      fg: color("fg")?,
      bg: color("bg")?,
      bold: flag("bold")?,
      italic: flag("italic")?,
      underline: flag("underline")?,
      reverse: flag("reverse")?,
    },
  })
}
