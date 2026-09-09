use std::{
  future::{Future, poll_fn},
  path::Path,
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
  task::Poll,
  time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use mlua::{
  Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, VmState, chunk::ChunkMode,
};
use serde_json::Value;
use tokio::sync::Mutex;

use super::{PluginManifest, PluginTool, bundle, host::Host};
use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

mod bindings;

pub(super) struct LuaRuntime {
  bundle: Arc<bundle::Bundle>,
  timeout: Duration,
  memory: usize,
  fuel: u64,
}

impl LuaRuntime {
  pub(super) async fn new(manifest: &PluginManifest, root: &Path, digest: &str) -> Result<Self> {
    let bundle = bundle::load(
      root,
      manifest
        .runtime
        .path
        .as_deref()
        .context("Lua path missing")?,
    )
    .await?;
    if bundle.digest != digest {
      bail!("Lua sources changed since the plugin was approved");
    }
    Ok(Self {
      bundle: Arc::new(bundle),
      timeout: Duration::from_millis(manifest.runtime.timeout_ms),
      memory: manifest.runtime.memory_bytes,
      fuel: manifest.runtime.fuel,
    })
  }

  async fn call(
    &self,
    definition: &PluginTool,
    context: &ToolContext,
    arguments: Value,
  ) -> Result<String> {
    let lua = Lua::new_with(
      StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
      LuaOptions::default(),
    )?;
    lua.set_memory_limit(self.memory)?;
    let used = Arc::new(AtomicU64::new(0));
    let counter = used.clone();
    lua.set_global_hook(
      HookTriggers::new().every_nth_instruction(1000),
      move |_, _| {
        counter.fetch_add(1000, Ordering::Relaxed);
        // yielding makes the deadline and cancellation observable even in a pure Lua loop
        Ok(VmState::Yield)
      },
    )?;
    let host = Arc::new(Mutex::new(Host::new(
      context.workspace.clone(),
      &definition.capabilities,
      self.timeout,
      context.max_output_bytes,
    )));
    bindings::install(&lua, host, context)?;
    bindings::modules(&lua, self.bundle.clone())?;
    for name in [
      "dofile",
      "loadfile",
      "load",
      "print",
      "warn",
      "collectgarbage",
      "coroutine",
    ] {
      lua.globals().set(name, mlua::Value::Nil)?;
    }

    let source = &self.bundle.files[&self.bundle.entry];
    let run = async {
      let exports: Table = lua
        .load(source)
        .set_name(&self.bundle.entry)
        .set_mode(ChunkMode::Text)
        .eval_async()
        .await?;
      let function: Function = exports.get(definition.name.as_str())?;
      let result: mlua::Value = function.call_async(lua.to_value(&arguments)?).await?;
      let text = match result {
        mlua::Value::String(text) => text.to_str()?.to_string(),
        value => {
          serde_json::to_string(&lua.from_value::<Value>(value)?).map_err(mlua::Error::external)?
        }
      };
      Ok::<_, mlua::Error>(truncate(text, context.max_output_bytes))
    };
    tokio::pin!(run);
    // the budget check is outside Lua, so pcall cannot catch it and keep executing
    let bounded = poll_fn(|cx| {
      if used.load(Ordering::Relaxed) >= self.fuel {
        return Poll::Ready(Err(mlua::Error::runtime("Lua instruction budget exceeded")));
      }
      let result = run.as_mut().poll(cx);
      if used.load(Ordering::Relaxed) >= self.fuel {
        return Poll::Ready(Err(mlua::Error::runtime("Lua instruction budget exceeded")));
      }
      result
    });
    Ok(
      tokio::time::timeout(self.timeout, bounded)
        .await
        .context("Lua tool timed out")??,
    )
  }
}

pub(super) struct LuaTool {
  runtime: Arc<LuaRuntime>,
  plugin: String,
  definition: PluginTool,
}
impl LuaTool {
  pub(super) fn new(runtime: Arc<LuaRuntime>, plugin: String, definition: PluginTool) -> Self {
    Self {
      runtime,
      plugin,
      definition,
    }
  }
}
#[async_trait]
impl Tool for LuaTool {
  fn spec(&self) -> ToolSpec {
    self.definition.spec(&self.plugin)
  }
  fn risk(&self, _: &Value) -> Risk {
    self.definition.risk()
  }
  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    self
      .runtime
      .call(&self.definition, context, arguments)
      .await
  }
}
