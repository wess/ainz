//! The person's own Lua: init.lua and the plugins beside it, run in one long-lived runtime the
//! way Neovim runs its configuration. Scripts set options, map keys, add commands, answer
//! events, draw panels and the status bar, and give the model tools. Without an init.lua none
//! of this exists, so it costs nothing.
//!
//! The runtime never touches the interface directly. What a script asks for is queued as an
//! [`Effect`] for the interface to carry out, and what it draws is kept here for the interface
//! to read when it next draws.

use std::{
  path::PathBuf,
  sync::{Arc, PoisonError, atomic::Ordering},
  time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use mlua::{Function, HookTriggers, Lua, LuaSerdeExt, Value, VmState};
use serde::Serialize;

use crate::{Config, hook::ToolGate, protocol::ToolCall, tool::Tool};

mod api;
pub mod files;
pub mod options;
mod registry;
pub mod text;
mod tool;

pub use files::{Project, Sources};
pub use text::{Color, Line, Segment, Style};

use registry::{Rhs, Shared};

/// How long a callback from the interface may run before it is stopped. A keymap or a status
/// item that loops for ever would otherwise take the terminal with it.
const CALLBACK: Duration = Duration::from_secs(2);
/// Loading init.lua and its plugins may reasonably do more.
const LOADING: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
  Info,
  Warn,
  Error,
}

/// Something a script asked the interface to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
  Notify {
    text: String,
    level: Level,
  },
  /// Typed into the prompt and sent: a prompt, steering during a run, or a slash command.
  Send(String),
  SetInput(String),
  Insert(String),
  Highlight {
    role: String,
    color: Color,
  },
  /// Tools changed after the agent was built.
  Rebuild,
  Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
  Left,
  Right,
}

/// A column a script draws beside the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
  pub id: u64,
  pub side: Side,
  pub width: u16,
  /// Rows to take from the column; `None` shares what is left.
  pub height: Option<u16>,
  pub title: String,
  pub lines: Vec<Line>,
  /// Show the end when there are more lines than rows, the way a log window does.
  pub follow: bool,
}

/// The interface as scripts see it: what `ainz.state()` returns, and what status items, the
/// status bar and the topic are drawn from.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct View {
  pub provider: String,
  pub model: String,
  pub mode: String,
  pub busy: bool,
  pub activity: String,
  pub input: String,
  pub tokens: Tokens,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub cost: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub context: Option<Context>,
  pub agents: Agents,
  pub todos: Count,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub branch: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub session: Option<String>,
  pub workspace: String,
  pub width: u16,
  pub height: u16,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Tokens {
  pub input: u64,
  pub output: u64,
  pub total: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Context {
  pub used: usize,
  pub limit: usize,
  pub percent: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Agents {
  pub running: usize,
  pub total: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Count {
  pub done: usize,
  pub total: usize,
}

/// What a key a script mapped does when it is pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
  /// Call the script's function; pass the id to [`Script::press`].
  Run(u64),
  /// Send this text, as if it were typed.
  Send(String),
}

/// A slash command a script added, for the palette and /help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
  pub name: String,
  pub desc: String,
}

pub struct Script {
  lua: Lua,
  shared: Arc<Shared>,
  sources: std::sync::Mutex<Sources>,
}

impl Script {
  /// A runtime with the API installed and nothing run yet.
  fn runtime(workspace: &std::path::Path, sources: &Sources) -> Result<(Lua, Arc<Shared>)> {
    let lua = Lua::new();
    let shared = Arc::new(Shared::new());
    let deadline = shared.clone();
    // a callback that runs too long is stopped, checked often enough to answer within a
    // moment and seldom enough to cost nothing measurable
    lua.set_global_hook(
      HookTriggers::new().every_nth_instruction(100_000),
      move |_, _| {
        let limit = deadline.deadline.load(Ordering::Relaxed);
        if limit != 0 && deadline.start.elapsed().as_nanos() as u64 > limit {
          return Err(mlua::Error::runtime(
            "the script ran too long and was stopped",
          ));
        }
        Ok(VmState::Continue)
      },
    )?;
    api::install(&lua, shared.clone(), workspace)?;
    let search: Vec<String> = sources
      .roots
      .iter()
      .flat_map(|root| {
        let lua = root.join("lua");
        [
          lua.join("?.lua").display().to_string(),
          lua.join("?/init.lua").display().to_string(),
        ]
      })
      .collect();
    if !search.is_empty() {
      let package: mlua::Table = lua.globals().get("package")?;
      let path: String = package.get("path")?;
      package.set("path", format!("{};{path}", search.join(";")))?;
    }
    Ok((lua, shared))
  }

  /// Runs init.lua and the plugins beside it against `config`, which takes whatever options
  /// they set. A script that fails is reported in the returned notes and the rest still run,
  /// so one broken plugin never stops Ainz from starting.
  pub fn load(
    sources: Sources,
    workspace: &std::path::Path,
    config: &mut Config,
  ) -> Result<(Self, Vec<String>)> {
    let (lua, shared) = Self::runtime(workspace, &sources)?;
    let script = Self {
      lua,
      shared,
      sources: std::sync::Mutex::new(sources),
    };
    let notes = script.source(config);
    Ok((script, notes))
  }

  fn source(&self, config: &mut Config) -> Vec<String> {
    let mut notes = Vec::new();
    if let Err(error) = self.sync(config).and_then(|()| self.prelude()) {
      notes.push(format!("lua: {error:#}"));
      return notes;
    }
    let sources = self.sources();
    for file in sources.files() {
      let result = std::fs::read_to_string(&file)
        .with_context(|| format!("read {}", file.display()))
        .and_then(|code| {
          self
            .guarded(LOADING, || {
              self
                .lua
                .load(&code)
                .set_name(format!("@{}", file.display()))
                .exec()
            })
            .map_err(|error| anyhow::anyhow!(short(&error)))
        });
      if let Err(error) = result {
        notes.push(format!("lua: {}: {error:#}", file.display()));
      }
    }
    if let Project::Untrusted(path) = &sources.project {
      notes.push(format!(
        "{} was not run because it is not trusted; /trust runs it from now on",
        path.display()
      ));
    }
    let (next, problems) = options::apply(config, self.writes());
    *config = next;
    notes.extend(problems);
    notes
  }

  fn prelude(&self) -> Result<()> {
    let ainz: mlua::Table = self.lua.globals().get("ainz")?;
    self
      .lua
      .load(include_str!("prelude.lua"))
      .set_name("=prelude")
      .call::<()>(ainz)?;
    Ok(())
  }

  /// Runs the scripts again, the way `:source` does in Neovim: everything they registered is
  /// dropped first, and the options they set now are applied to the configuration as it
  /// stands. An option a script no longer sets keeps its value until Ainz starts again.
  pub fn reload(&self, sources: Sources, config: &mut Config) -> Vec<String> {
    *self.sources.lock().unwrap_or_else(PoisonError::into_inner) = sources;
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .clear();
    self.writes();
    let notes = self.source(config);
    self.effect(Effect::Rebuild);
    self.bump();
    notes
  }

  pub fn sources(&self) -> Sources {
    self
      .sources
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .clone()
  }

  /// Gives scripts the configuration as it now stands, so `ainz.opt` reads what /settings or
  /// a command just changed. Call it after anything that may have changed the configuration.
  pub fn sync(&self, config: &Config) -> Result<()> {
    let value = self.lua.to_value_with(config, serialize())?;
    let ainz: mlua::Table = self.lua.globals().get("ainz")?;
    ainz.set("__opt", value)?;
    Ok(())
  }

  /// Describes the interface for `ainz.state()` and the next status items.
  pub fn describe(&self, view: View) {
    let mut current = self
      .shared
      .view
      .lock()
      .unwrap_or_else(PoisonError::into_inner);
    if *current != view {
      *current = view;
    }
  }

  /// Option writes since the last call, applied to `config`. `None` when there were none.
  pub fn options(&self, config: &Config) -> Option<(Config, Vec<String>)> {
    let writes = self.writes();
    (!writes.is_empty()).then(|| options::apply(config, writes))
  }

  fn writes(&self) -> Vec<(Vec<serde_json::Value>, serde_json::Value)> {
    std::mem::take(
      &mut *self
        .shared
        .writes
        .lock()
        .unwrap_or_else(PoisonError::into_inner),
    )
  }

  /// What scripts have asked for since the last call.
  pub fn effects(&self) -> Vec<Effect> {
    std::mem::take(
      &mut *self
        .shared
        .effects
        .lock()
        .unwrap_or_else(PoisonError::into_inner),
    )
  }

  /// What scripts printed or reported, leaving everything else they asked for in place: what
  /// loading said, before there is an interface to carry out the rest.
  pub fn take_notices(&self) -> Vec<(String, Level)> {
    let mut effects = self
      .shared
      .effects
      .lock()
      .unwrap_or_else(PoisonError::into_inner);
    let mut notices = Vec::new();
    effects.retain(|effect| match effect {
      Effect::Notify { text, level } => {
        notices.push((text.clone(), *level));
        false
      }
      _ => true,
    });
    notices
  }

  fn effect(&self, effect: Effect) {
    self
      .shared
      .effects
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .push(effect);
  }

  /// Whether a panel, the bar or the topic changed since the last call.
  pub fn take_redraw(&self) -> bool {
    self.shared.redraw.swap(false, Ordering::Relaxed)
  }

  fn bump(&self) {
    self.shared.generation.fetch_add(1, Ordering::Relaxed);
    self.shared.redraw.store(true, Ordering::Relaxed);
  }

  /// Runs `call` with a deadline, so a loop that never ends is stopped rather than hanging
  /// whatever called it.
  fn guarded<R>(&self, limit: Duration, call: impl FnOnce() -> mlua::Result<R>) -> mlua::Result<R> {
    let at = self.shared.start.elapsed() + limit;
    self
      .shared
      .deadline
      .store(at.as_nanos() as u64, Ordering::Relaxed);
    let result = call();
    self.shared.deadline.store(0, Ordering::Relaxed);
    result
  }

  /// Calls a script's function, reporting a failure in the buffer rather than to the caller.
  fn call<R: mlua::FromLuaMulti>(
    &self,
    function: &Function,
    args: impl mlua::IntoLuaMulti,
  ) -> Option<R> {
    match self.guarded(CALLBACK, || function.call::<R>(args)) {
      Ok(value) => Some(value),
      Err(error) => {
        self.effect(Effect::Notify {
          text: format!("lua: {}", short(&error)),
          level: Level::Error,
        });
        None
      }
    }
  }

  fn handlers(&self, event: &str) -> Vec<Function> {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .handlers
      .get(event)
      .map(|handlers| {
        handlers
          .iter()
          .map(|(_, function)| function.clone())
          .collect()
      })
      .unwrap_or_default()
  }

  /// Whether anything listens for `event`, so a caller can skip building what it would carry.
  pub fn handles(&self, event: &str) -> bool {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .handlers
      .get(event)
      .is_some_and(|handlers| !handlers.is_empty())
  }

  /// Tells every handler of `event`. Nothing is converted unless something is listening.
  pub fn emit<T: Serialize>(&self, event: &str, payload: &T) {
    let handlers = self.handlers(event);
    if handlers.is_empty() {
      return;
    }
    let Ok(value) = self.lua.to_value_with(payload, serialize()) else {
      return;
    };
    for handler in handlers {
      self.call::<()>(&handler, value.clone());
    }
  }

  /// A prompt as the `prompt` handlers leave it: each may return new text, or false to keep it
  /// from being sent at all.
  pub fn prompt(&self, text: String) -> Option<String> {
    let mut text = text;
    for handler in self.handlers("prompt") {
      let event = self.lua.create_table().ok()?;
      event.set("text", text.as_str()).ok()?;
      match self.call::<Value>(&handler, event) {
        Some(Value::String(replaced)) => text = replaced.to_string_lossy(),
        Some(Value::Boolean(false)) => return None,
        _ => {}
      }
    }
    Some(text)
  }

  /// The check the agent makes before every tool call: a `pre_tool` handler that returns
  /// false, with a reason if it likes, refuses the call in every permission mode.
  pub fn gate(self: &Arc<Self>) -> ToolGate {
    let script = self.clone();
    Arc::new(move |call: &ToolCall| script.pre_tool(call))
  }

  fn pre_tool(&self, call: &ToolCall) -> Result<(), String> {
    let handlers = self.handlers("pre_tool");
    if handlers.is_empty() {
      return Ok(());
    }
    #[derive(Serialize)]
    struct Payload<'a> {
      id: &'a str,
      name: &'a str,
      arguments: &'a serde_json::Value,
      #[serde(skip_serializing_if = "Option::is_none")]
      subject: Option<&'a str>,
    }
    let payload = Payload {
      id: &call.id,
      name: &call.name,
      arguments: &call.arguments,
      subject: crate::agent::subject(&call.arguments),
    };
    let value = self
      .lua
      .to_value_with(&payload, serialize())
      .map_err(|error| error.to_string())?;
    for handler in handlers {
      let refused = match self.guarded(CALLBACK, || {
        handler.call::<(Value, Option<String>)>(value.clone())
      }) {
        Ok((Value::Boolean(false), reason)) => {
          Some(reason.unwrap_or_else(|| "a script refused it".into()))
        }
        Ok(_) => None,
        // a gate that fails closes: a broken check is not a passed one
        Err(error) => Some(format!("pre_tool handler failed: {}", short(&error))),
      };
      if let Some(reason) = refused {
        return Err(reason);
      }
    }
    Ok(())
  }

  /// The keys scripts mapped, in the order they were mapped.
  pub fn keymaps(&self) -> Vec<(String, Key, String)> {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .keymaps
      .iter()
      .map(|keymap| {
        let key = match &keymap.rhs {
          Rhs::Function(_) => Key::Run(keymap.id),
          Rhs::Text(text) => Key::Send(text.clone()),
        };
        (keymap.keys.clone(), key, keymap.desc.clone())
      })
      .collect()
  }

  /// Runs the function mapped as `id`.
  pub fn press(&self, id: u64) {
    let function = self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .keymaps
      .iter()
      .find(|keymap| keymap.id == id)
      .and_then(|keymap| match &keymap.rhs {
        Rhs::Function(function) => Some(function.clone()),
        Rhs::Text(_) => None,
      });
    if let Some(function) = function {
      self.call::<()>(&function, ());
    }
  }

  pub fn commands(&self) -> Vec<Command> {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .commands
      .iter()
      .map(|(name, command)| Command {
        name: name.clone(),
        desc: command.desc.clone(),
      })
      .collect()
  }

  /// Runs `/name args` when a script defined `name`. False means no script did.
  pub fn command(&self, name: &str, args: &str) -> bool {
    let function = self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .commands
      .get(name)
      .map(|command| command.function.clone());
    let Some(function) = function else {
      return false;
    };
    let words: Vec<&str> = args.split_whitespace().collect();
    if let Ok(event) = self.lua.create_table() {
      drop(event.set("args", args));
      drop(event.set("fargs", words));
      drop(event.set("name", name));
      self.call::<()>(&function, event);
    }
    true
  }

  /// When the next timer is due; `None` when no timer is waiting, so an idle session sleeps.
  pub fn next_timer(&self) -> Option<Instant> {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .timers
      .values()
      .map(|timer| timer.due)
      .min()
  }

  /// Runs every timer that is due, and schedules the repeating ones again.
  pub fn fire_timers(&self) {
    let now = Instant::now();
    let due: Vec<Function> = {
      let mut registry = self
        .shared
        .registry
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
      let ids: Vec<u64> = registry
        .timers
        .iter()
        .filter(|(_, timer)| timer.due <= now)
        .map(|(id, _)| *id)
        .collect();
      ids
        .into_iter()
        .filter_map(|id| {
          let timer = registry.timers.get_mut(&id)?;
          let function = timer.function.clone();
          if timer.repeat {
            // a timer that fell behind runs once, not once for every interval it missed
            timer.due = now + timer.every;
          } else {
            registry.timers.remove(&id);
          }
          Some(function)
        })
        .collect()
    };
    if due.is_empty() {
      return;
    }
    for function in due {
      self.call::<()>(&function, ());
    }
    self.bump();
  }

  pub fn panels(&self) -> Vec<Panel> {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .panels
      .values()
      .cloned()
      .collect()
  }

  /// The status bar a script draws in full, when one does.
  pub fn statusline(&self) -> Option<Line> {
    let function = self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .statusline
      .clone()?;
    self.styled(&function)
  }

  /// The topic line a script draws, when one does.
  pub fn topic(&self) -> Option<Line> {
    let function = self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .topic
      .clone()?;
    self.styled(&function)
  }

  /// The item names a script put on the bar, built-in and its own, in order.
  pub fn bar(&self) -> Option<Vec<String>> {
    self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .bar
      .clone()
  }

  /// A status item a script defined, drawn now; `None` when no script defined `name`.
  pub fn item(&self, name: &str) -> Option<Line> {
    let function = self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .items
      .get(name)
      .cloned()?;
    Some(self.styled(&function).unwrap_or_default())
  }

  fn styled(&self, function: &Function) -> Option<Line> {
    let view = self
      .shared
      .view
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .clone();
    let state = self.lua.to_value_with(&view, serialize()).ok()?;
    let value = self.call::<Value>(function, state)?;
    api::line(&value).ok()
  }

  /// The tools scripts gave the model.
  pub fn tools(self: &Arc<Self>) -> Vec<Arc<dyn Tool>> {
    let names: Vec<String> = self
      .shared
      .registry
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .tools
      .keys()
      .cloned()
      .collect();
    names
      .into_iter()
      .map(|name| Arc::new(tool::LuaTool::new(self.clone(), name)) as Arc<dyn Tool>)
      .collect()
  }

  /// Runs a chunk typed at /lua: a statement, or an expression whose value is shown.
  pub fn run(&self, code: &str) -> Result<String> {
    let inspect: Function = self
      .lua
      .globals()
      .get::<mlua::Table>("ainz")?
      .get("inspect")?;
    let expression = format!("return {code}");
    let chunk = match self.lua.load(&expression).set_name("=lua").into_function() {
      Ok(function) => function,
      Err(_) => self
        .lua
        .load(code)
        .set_name("=lua")
        .into_function()
        .map_err(|error| anyhow::anyhow!(short(&error)))?,
    };
    let values = self
      .guarded(CALLBACK, || chunk.call::<mlua::MultiValue>(()))
      .map_err(|error| anyhow::anyhow!(short(&error)))?;
    let shown: Vec<String> = values
      .into_iter()
      .map(|value| inspect.call::<String>(value).unwrap_or_default())
      .collect();
    Ok(shown.join("\t"))
  }

  /// The files that were run, for /lua and the doctor.
  pub fn files(&self) -> Vec<PathBuf> {
    self.sources().files()
  }
}

fn serialize() -> mlua::serde::SerializeOptions {
  // nil rather than a null sentinel, which Lua would treat as true
  mlua::serde::SerializeOptions::new()
    .serialize_none_to_null(false)
    .serialize_unit_to_null(false)
}

/// The first lines of a Lua error, which say what went wrong and where; the rest of a
/// traceback is Rust's side of the call.
fn short(error: &mlua::Error) -> String {
  let text = error.to_string();
  let lines: Vec<&str> = text
    .lines()
    .filter(|line| {
      !line.trim().is_empty() && !line.contains("[C]:") && !line.contains("stack traceback")
    })
    .take(3)
    .collect();
  lines.join(" · ")
}

#[cfg(test)]
#[path = "../../tests/script/unit.rs"]
mod tests;
