use std::{
  collections::BTreeSet,
  path::{Path, PathBuf},
  process::Stdio,
  sync::Arc,
  time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::{
  fs,
  io::{AsyncRead, AsyncReadExt},
  process::Command,
  time::timeout,
};
use wasmtime::{
  Config, Engine, Store, StoreLimits, StoreLimitsBuilder,
  component::{Component, HasData, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::{Capability, PluginManifest, PluginTool};
use crate::{
  protocol::ToolSpec,
  tool::{Risk, Tool, ToolContext, truncate},
  workspace,
};

wasmtime::component::bindgen!({
  path: "wit",
  world: "plugin",
  exports: { default: async },
  imports: {
    "agentx:plugin/host.read-file": async,
    "agentx:plugin/host.write-file": async,
    "agentx:plugin/host.run": async,
    "agentx:plugin/host.fetch": async,
  },
});

pub(super) struct ComponentRuntime {
  engine: Engine,
  component: Component,
  timeout: Duration,
  memory_bytes: usize,
  fuel: u64,
}

impl ComponentRuntime {
  pub(super) fn new(manifest: &PluginManifest, root: &Path) -> Result<Self> {
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
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)?;
    let component = Component::from_file(&engine, &path)
      .map_err(|error| anyhow::anyhow!("compile component {}: {error}", path.display()))?;
    Ok(Self {
      engine,
      component,
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
    let mut linker = Linker::new(&self.engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    Plugin::add_to_linker::<HostState, HostBindings>(&mut linker, |state| state)?;
    let bindings = Plugin::instantiate_async(&mut store, &self.component, &linker).await?;
    let result = timeout(
      self.timeout,
      bindings.call_call(&mut store, tool, arguments),
    )
    .await
    .context("component timed out")??;
    result.map_err(|error| anyhow::anyhow!("component error: {error}"))
  }
}

struct HostBindings;

impl HasData for HostBindings {
  type Data<'a> = &'a mut HostState;
}

impl agentx::plugin::host::Host for HostState {
  async fn read_file(&mut self, path: String) -> Result<String, String> {
    self
      .host_read(&path)
      .await
      .map_err(|error| format!("{error:#}"))
  }

  async fn write_file(&mut self, path: String, content: String) -> Result<(), String> {
    self
      .host_write(&path, &content)
      .await
      .map_err(|error| format!("{error:#}"))
  }

  async fn run(&mut self, command: String) -> Result<String, String> {
    self
      .host_run(&command)
      .await
      .map_err(|error| format!("{error:#}"))
  }

  async fn fetch(&mut self, url: String) -> Result<String, String> {
    self
      .host_fetch(&url)
      .await
      .map_err(|error| format!("{error:#}"))
  }
}

struct HostState {
  wasi: WasiCtx,
  table: ResourceTable,
  limits: StoreLimits,
  workspace: PathBuf,
  capabilities: BTreeSet<Capability>,
  timeout: Duration,
  max_output_bytes: usize,
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
      workspace,
      capabilities: capabilities.iter().copied().collect(),
      timeout,
      max_output_bytes,
    }
  }

  fn require(&self, capability: Capability) -> Result<()> {
    if !self.capabilities.contains(&capability) {
      bail!("{capability:?} capability is required");
    }
    Ok(())
  }

  async fn host_read(&mut self, input: &str) -> Result<String> {
    self.require(Capability::WorkspaceRead)?;
    let path = workspace::existing(&self.workspace, input).await?;
    let file = fs::File::open(&path).await?;
    let mut bytes = Vec::new();
    file
      .take(self.max_output_bytes.saturating_add(1) as u64)
      .read_to_end(&mut bytes)
      .await?;
    if bytes.len() > self.max_output_bytes {
      bail!("file exceeds the host transfer limit");
    }
    String::from_utf8(bytes).context("file was not UTF-8")
  }

  async fn host_write(&mut self, input: &str, content: &str) -> Result<()> {
    self.require(Capability::WorkspaceWrite)?;
    if content.len() > self.max_output_bytes {
      bail!("content exceeds the host transfer limit");
    }
    let path = workspace::writable(&self.workspace, input).await?;
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).await?;
    }
    fs::write(path, content).await?;
    Ok(())
  }

  async fn host_run(&mut self, command: &str) -> Result<String> {
    self.require(Capability::ProcessExec)?;
    let mut child = Command::new("sh")
      .args(["-lc", command])
      .current_dir(&self.workspace)
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .spawn()?;
    let stdout = child.stdout.take().context("command stdout unavailable")?;
    let stderr = child.stderr.take().context("command stderr unavailable")?;
    let run = async {
      let (status, stdout, stderr) = tokio::try_join!(
        child.wait(),
        capture(stdout, self.max_output_bytes),
        capture(stderr, self.max_output_bytes)
      )?;
      if stdout.truncated || stderr.truncated {
        bail!("command output exceeded the host transfer limit");
      }
      let mut output = String::from_utf8(stdout.bytes).context("stdout was not UTF-8")?;
      output.push_str(&String::from_utf8(stderr.bytes).context("stderr was not UTF-8")?);
      output.push_str(&format!("\n[exit {}]", status.code().unwrap_or(-1)));
      Result::<String>::Ok(output)
    };
    timeout(self.timeout, run)
      .await
      .context("command timed out")?
  }

  async fn host_fetch(&mut self, url: &str) -> Result<String> {
    self.require(Capability::Network)?;
    let url = reqwest::Url::parse(url)?;
    if !matches!(url.scheme(), "http" | "https") {
      bail!("only HTTP and HTTPS URLs are supported");
    }
    let response = reqwest::Client::new()
      .get(url)
      .timeout(self.timeout)
      .send()
      .await?
      .error_for_status()?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
      let chunk = chunk?;
      if bytes.len().saturating_add(chunk.len()) > self.max_output_bytes {
        bail!("response exceeded the host transfer limit");
      }
      bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).context("response was not UTF-8")
  }
}

struct Capture {
  bytes: Vec<u8>,
  truncated: bool,
}

async fn capture(mut reader: impl AsyncRead + Unpin, limit: usize) -> std::io::Result<Capture> {
  let mut bytes = Vec::with_capacity(limit.min(8192));
  let mut buffer = [0_u8; 8192];
  let mut truncated = false;
  loop {
    let read = reader.read(&mut buffer).await?;
    if read == 0 {
      break;
    }
    let remaining = limit.saturating_sub(bytes.len());
    bytes.extend_from_slice(&buffer[..read.min(remaining)]);
    truncated |= read > remaining;
  }
  Ok(Capture { bytes, truncated })
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
    ToolSpec {
      name: format!("{}_{}", self.plugin, self.definition.name),
      description: self.definition.description.clone(),
      parameters: serde_json::to_value(&self.definition.parameters)
        .unwrap_or_else(|_| json!({"type": "object"})),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    self
      .definition
      .capabilities
      .iter()
      .map(|capability| capability.risk())
      .max()
      .unwrap_or(Risk::Read)
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
    if output.len() > context.max_output_bytes {
      return Ok(truncate(output, context.max_output_bytes));
    }
    if output.is_empty() {
      bail!("component returned an empty response");
    }
    Ok(output)
  }
}
