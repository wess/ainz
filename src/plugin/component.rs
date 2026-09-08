use std::{
  path::{Path, PathBuf},
  sync::{Arc, LazyLock},
  time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::Value;
use tokio::time::timeout;
use wasmtime::{
  Config, Engine, Store, StoreLimits, StoreLimitsBuilder,
  component::{Component, HasData, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::{Capability, PluginManifest, PluginTool, catalog::read_artifact, host::Host};
use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
};

wasmtime::component::bindgen!({
  path: "wit",
  world: "plugin",
  exports: { default: async },
  imports: {
    "ainz:plugin/host.read-file": async,
    "ainz:plugin/host.write-file": async,
    "ainz:plugin/host.run": async,
    "ainz:plugin/host.fetch": async,
  },
});

const EPOCH_TICK: Duration = Duration::from_millis(50);

// one engine for every component; its epoch ticker is what lets a spinning guest be timed out
static ENGINE: LazyLock<Result<Engine, String>> = LazyLock::new(|| {
  let mut config = Config::new();
  config.consume_fuel(true);
  config.epoch_interruption(true);
  let engine = Engine::new(&config).map_err(|error| error.to_string())?;
  let ticker = engine.clone();
  std::thread::spawn(move || {
    loop {
      std::thread::sleep(EPOCH_TICK);
      ticker.increment_epoch();
    }
  });
  Ok(engine)
});

pub(super) struct ComponentRuntime {
  engine: Engine,
  pre: PluginPre<HostState>,
  timeout: Duration,
  memory_bytes: usize,
  fuel: u64,
}

impl ComponentRuntime {
  pub(super) async fn new(manifest: &PluginManifest, root: &Path, digest: &str) -> Result<Self> {
    let path = manifest
      .runtime
      .path
      .as_ref()
      .context("component path is missing")?;
    let path = if path.is_relative() {
      root.join(path)
    } else {
      path.clone()
    };
    // compiled from the very bytes that were hashed, so the approval covers what runs
    let bytes = read_artifact(&path).await?;
    if super::catalog::digest(&bytes) != digest {
      bail!("{} changed since the plugin was approved", path.display());
    }
    let engine = ENGINE
      .as_ref()
      .map(Engine::clone)
      .map_err(|error| anyhow!("create wasm engine: {error}"))?;
    let component = Component::new(&engine, &bytes)
      .map_err(anyhow::Error::from)
      .with_context(|| format!("compile component {}", path.display()))?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    Plugin::add_to_linker::<HostState, HostBindings>(&mut linker, |state| state)?;
    let pre = PluginPre::new(linker.instantiate_pre(&component)?)?;
    Ok(Self {
      engine,
      pre,
      timeout: Duration::from_millis(manifest.runtime.timeout_ms),
      memory_bytes: manifest.runtime.memory_bytes,
      fuel: manifest.runtime.fuel,
    })
  }

  async fn call(
    &self,
    tool: &str,
    arguments: &str,
    context: &ToolContext,
    capabilities: &[Capability],
  ) -> Result<String> {
    let state = HostState::new(
      self.memory_bytes,
      context.workspace.clone(),
      capabilities,
      self.timeout,
      context.max_output_bytes,
    );
    let mut store = Store::new(&self.engine, state);
    store.limiter(|state| &mut state.limits);
    store.set_fuel(self.fuel)?;
    // every epoch tick the guest yields, which is when the wall-clock timeout can fire
    store.set_epoch_deadline(1);
    store.epoch_deadline_async_yield_and_update(1);
    let bindings = self.pre.instantiate_async(&mut store).await?;
    let result = timeout(
      self.timeout,
      bindings.call_call(&mut store, tool, arguments),
    )
    .await
    .context("component timed out")??;
    result.map_err(|error| anyhow!("component error: {error}"))
  }
}

struct HostBindings;

impl HasData for HostBindings {
  type Data<'a> = &'a mut HostState;
}

impl ainz::plugin::host::Host for HostState {
  async fn read_file(&mut self, path: String) -> Result<String, String> {
    self
      .host
      .host_read(&path)
      .await
      .map_err(|error| format!("{error:#}"))
  }

  async fn write_file(&mut self, path: String, content: String) -> Result<(), String> {
    self
      .host
      .host_write(&path, &content)
      .await
      .map_err(|error| format!("{error:#}"))
  }

  async fn run(&mut self, command: String) -> Result<String, String> {
    self
      .host
      .host_run(&command)
      .await
      .map_err(|error| format!("{error:#}"))
  }

  async fn fetch(&mut self, url: String) -> Result<String, String> {
    self
      .host
      .host_fetch(&url)
      .await
      .map_err(|error| format!("{error:#}"))
  }
}

struct HostState {
  wasi: WasiCtx,
  table: ResourceTable,
  limits: StoreLimits,
  host: Host,
}

impl HostState {
  fn new(
    memory_bytes: usize,
    workspace: PathBuf,
    capabilities: &[Capability],
    timeout: Duration,
    max_output_bytes: usize,
  ) -> Self {
    Self {
      wasi: WasiCtxBuilder::new().build(),
      table: ResourceTable::new(),
      limits: StoreLimitsBuilder::new()
        .memory_size(memory_bytes)
        .instances(4)
        .memories(4)
        .tables(4)
        .trap_on_grow_failure(true)
        .build(),
      host: Host::new(workspace, capabilities, timeout, max_output_bytes),
    }
  }
}

impl WasiView for HostState {
  fn ctx(&mut self) -> WasiCtxView<'_> {
    WasiCtxView {
      ctx: &mut self.wasi,
      table: &mut self.table,
    }
  }
}

pub(super) struct ComponentTool {
  runtime: Arc<ComponentRuntime>,
  plugin: String,
  definition: PluginTool,
}

impl ComponentTool {
  pub(super) fn new(
    runtime: Arc<ComponentRuntime>,
    plugin: String,
    definition: PluginTool,
  ) -> Self {
    Self {
      runtime,
      plugin,
      definition,
    }
  }
}

#[async_trait]
impl Tool for ComponentTool {
  fn spec(&self) -> ToolSpec {
    self.definition.spec(&self.plugin)
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    self.definition.risk()
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let output = self
      .runtime
      .call(
        &self.definition.name,
        &arguments.to_string(),
        context,
        &self.definition.capabilities,
      )
      .await?;
    Ok(truncate(output, context.max_output_bytes))
  }
}
