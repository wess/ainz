use anyhow::{Result, ensure};

use crate::{Config, HookRunner, PermissionMode, PermissionRules};

#[derive(Clone)]
pub struct RunOptions {
  pub instructions: String,
  pub permissions: PermissionMode,
  // what may run without asking, whatever the mode
  pub rules: PermissionRules,
  pub max_steps: usize,
  pub max_output_bytes: usize,
  pub context_tokens: usize,
  pub compact_at_tokens: usize,
  pub preserve_messages: usize,
  // asked of the model once a compaction has archived messages, when memory is on
  pub memory_nudge: Option<String>,
  // empty by default, so a session with no [hooks] configured spawns nothing
  pub hooks: HookRunner,
}

impl Default for RunOptions {
  fn default() -> Self {
    Self::from(&Config::default())
  }
}

impl From<&Config> for RunOptions {
  /// Copies run limits and policy without loading configuration, credentials, or extensions.
  /// The host supplies instructions and any memory guidance explicitly.
  fn from(config: &Config) -> Self {
    Self {
      instructions: String::new(),
      permissions: config.permissions,
      rules: config.rules.clone(),
      max_steps: config.max_steps,
      max_output_bytes: config.max_output_bytes,
      context_tokens: config.context_tokens,
      compact_at_tokens: config.compact_at_tokens,
      preserve_messages: config.preserve_messages,
      memory_nudge: None,
      hooks: HookRunner::new(config.hooks.clone()),
    }
  }
}

impl RunOptions {
  /// Checked before a run changes the session or starts hooks and providers.
  pub fn validate(&self) -> Result<()> {
    ensure!(self.max_steps > 0, "max_steps must be greater than zero");
    ensure!(
      self.compact_at_tokens > 0 && self.compact_at_tokens < self.context_tokens,
      "compact_at_tokens must be greater than zero and below context_tokens"
    );
    ensure!(
      self.preserve_messages >= 2,
      "preserve_messages must be at least two"
    );
    Ok(())
  }
}
