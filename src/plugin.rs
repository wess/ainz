use std::path::PathBuf;

pub(crate) use crate::output::capture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{protocol::ToolSpec, tool::Risk};

mod bundle;
mod catalog;
#[cfg(feature = "wasm")]
mod component;
#[cfg(any(feature = "lua", feature = "wasm"))]
mod host;
#[cfg(feature = "lua")]
mod lua;
mod process;

pub use catalog::{DiscoveredPlugin, PluginCatalog};

// host-side ceilings a manifest cannot raise
pub const MAX_TIMEOUT_MS: u64 = 300_000;
pub const MAX_MEMORY_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFormat {
  Ainz,
  AgentPlugin,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginManifest {
  pub plugin: PluginMeta,
  pub runtime: PluginRuntime,
  #[serde(default)]
  pub capabilities: Vec<Capability>,
  #[serde(default)]
  pub tools: Vec<PluginTool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginMeta {
  pub name: String,
  pub version: String,
  #[serde(default = "api_version")]
  pub api: u32,
  #[serde(default = "enabled")]
  pub enabled: bool,
}

fn enabled() -> bool {
  true
}

fn api_version() -> u32 {
  1
}

// memory_bytes and fuel apply to sandboxed runtimes; command only to processes
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginRuntime {
  #[serde(default)]
  pub kind: RuntimeKind,
  #[serde(default)]
  pub command: Vec<String>,
  pub path: Option<PathBuf>,
  #[serde(default = "default_timeout")]
  pub timeout_ms: u64,
  #[serde(default = "default_memory")]
  pub memory_bytes: usize,
  #[serde(default = "default_fuel")]
  pub fuel: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
  #[default]
  Process,
  Component,
  Lua,
}

fn default_timeout() -> u64 {
  30_000
}

fn default_memory() -> usize {
  64 * 1024 * 1024
}

fn default_fuel() -> u64 {
  10_000_000
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
  Compute,
  WorkspaceRead,
  WorkspaceWrite,
  ProcessExec,
  Network,
}

impl Capability {
  fn risk(self) -> Risk {
    match self {
      Self::Compute | Self::WorkspaceRead => Risk::Read,
      Self::WorkspaceWrite => Risk::Write,
      Self::ProcessExec => Risk::Execute,
      Self::Network => Risk::Network,
    }
  }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginTool {
  pub name: String,
  pub description: String,
  pub capabilities: Vec<Capability>,
  pub parameters: Value,
}

impl PluginTool {
  fn spec(&self, plugin: &str) -> ToolSpec {
    ToolSpec {
      name: format!("{plugin}_{}", self.name),
      description: self.description.clone(),
      parameters: self.parameters.clone(),
    }
  }

  fn risk(&self) -> Risk {
    self
      .capabilities
      .iter()
      .map(|capability| capability.risk())
      .max()
      .unwrap_or(Risk::Read)
  }
}
