use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::tool::Risk;

mod catalog;
mod component;
mod process;

pub use catalog::{DiscoveredPlugin, PluginCatalog};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFormat {
  AgentX,
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
  pub(super) fn risk(self) -> Risk {
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
  pub parameters: toml::Value,
}
