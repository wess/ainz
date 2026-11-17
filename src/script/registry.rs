//! What scripts have registered: keymaps, commands, event handlers, timers, panels, status bar
//! items, and tools. Lua functions are kept here and cloned out before they are called, so a
//! callback that registers something else never waits on its own lock.

use std::{
  collections::BTreeMap,
  sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64},
  },
  time::{Duration, Instant},
};

use mlua::Function;
use serde_json::Value;

use super::{Effect, Panel, View};
use crate::tool::Risk;

pub(super) enum Rhs {
  Function(Function),
  /// Typed into the prompt and sent, so it may be a prompt or a slash command.
  Text(String),
}

pub(super) struct Keymap {
  pub id: u64,
  pub keys: String,
  pub rhs: Rhs,
  pub desc: String,
}

pub(super) struct Command {
  pub function: Function,
  pub desc: String,
}

pub(super) struct Timer {
  pub every: Duration,
  pub due: Instant,
  pub repeat: bool,
  pub function: Function,
}

pub(super) struct ToolDef {
  pub description: String,
  pub parameters: Value,
  pub risk: Risk,
  pub function: Function,
}

#[derive(Default)]
pub(super) struct Registry {
  pub next: u64,
  pub keymaps: Vec<Keymap>,
  pub commands: BTreeMap<String, Command>,
  pub handlers: BTreeMap<String, Vec<(u64, Function)>>,
  pub timers: BTreeMap<u64, Timer>,
  pub panels: BTreeMap<u64, Panel>,
  pub items: BTreeMap<String, Function>,
  pub bar: Option<Vec<String>>,
  pub statusline: Option<Function>,
  pub topic: Option<Function>,
  pub tools: BTreeMap<String, ToolDef>,
}

impl Registry {
  pub fn id(&mut self) -> u64 {
    self.next += 1;
    self.next
  }

  /// Everything a reload starts over from. Ids keep counting, so a handle from before the
  /// reload never names something registered after it.
  pub fn clear(&mut self) {
    let next = self.next;
    *self = Self {
      next,
      ..Self::default()
    };
  }
}

/// State shared between the runtime and the functions it gives Lua.
pub(super) struct Shared {
  pub registry: Mutex<Registry>,
  pub effects: Mutex<Vec<Effect>>,
  /// Option writes, as a path into the configuration and the value written there.
  pub writes: Mutex<Vec<(Vec<Value>, Value)>>,
  /// What `ainz.state()` returns: the interface as it was last described to the runtime.
  pub view: Mutex<View>,
  /// Bumped whenever something a status item or panel shows may have changed.
  pub generation: AtomicU64,
  /// Nanoseconds after `start` at which a running callback is stopped; zero is none.
  pub deadline: AtomicU64,
  pub start: Instant,
  /// Set once a panel, the bar or the topic changes, and taken by the interface.
  pub redraw: AtomicBool,
}

impl Shared {
  pub fn new() -> Self {
    Self {
      registry: Mutex::default(),
      effects: Mutex::default(),
      writes: Mutex::default(),
      view: Mutex::default(),
      generation: AtomicU64::new(0),
      deadline: AtomicU64::new(0),
      start: Instant::now(),
      redraw: AtomicBool::new(false),
    }
  }
}
