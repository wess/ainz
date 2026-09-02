use std::{
  collections::BTreeMap,
  future::Future,
  pin::Pin,
  sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicUsize, Ordering},
  },
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::{
  protocol::{ToolSpec, Usage},
  tool::{Risk, Tool, ToolContext, truncate},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubagentResult {
  pub session_id: Uuid,
  pub name: String,
  pub output: String,
  pub usage: Usage,
}

#[derive(Clone, Debug)]
pub struct SubagentRequest {
  pub parent_id: Uuid,
  pub name: String,
  pub prompt: String,
  pub role: Option<String>,
}

pub type SubagentFuture = Pin<Box<dyn Future<Output = Result<SubagentResult>> + Send + 'static>>;
pub type SubagentHandler = Arc<dyn Fn(SubagentRequest) -> SubagentFuture + Send + Sync>;

// the guardians of each floor of Nazarick, in floor order, so a roster of running
// subagents reads as names rather than truncated session ids
const GUARDIANS: [&str; 10] = [
  "shalltear",
  "gargantua",
  "cocytus",
  "aura",
  "mare",
  "demiurge",
  "victim",
  "albedo",
  "sebas",
  "pandora",
];

// past the tenth the list repeats with a suffix, so names stay unique for long runs
pub fn guardian(index: usize) -> String {
  let name = GUARDIANS[index % GUARDIANS.len()];
  let round = index / GUARDIANS.len();
  if round == 0 {
    name.into()
  } else {
    format!("{name}-{}", round + 1)
  }
}

/// Names delegations and keeps the background ones addressable until they are collected.
pub struct SubagentRegistry {
  handler: SubagentHandler,
  spawned: AtomicUsize,
  running: Mutex<BTreeMap<String, JoinHandle<Result<SubagentResult>>>>,
}

impl SubagentRegistry {
  pub fn new(handler: SubagentHandler) -> Arc<Self> {
    Arc::new(Self {
      handler,
      spawned: AtomicUsize::new(0),
      running: Mutex::new(BTreeMap::new()),
    })
  }

  pub fn next_name(&self) -> String {
    guardian(self.spawned.fetch_add(1, Ordering::Relaxed))
  }

  fn tasks(
    &self,
  ) -> std::sync::MutexGuard<'_, BTreeMap<String, JoinHandle<Result<SubagentResult>>>> {
    self.running.lock().unwrap_or_else(PoisonError::into_inner)
  }

  pub async fn run(&self, request: SubagentRequest) -> Result<SubagentResult> {
    (self.handler)(request).await
  }

  /// Start a delegation without waiting for it. The caller gets the guardian's name back and
  /// collects the result later, so several can work at once.
  pub fn start(&self, request: SubagentRequest) -> String {
    let name = request.name.clone();
    let task = (self.handler)(request);
    self.tasks().insert(name.clone(), tokio::spawn(task));
    name
  }

  pub async fn collect(&self, name: &str) -> Result<SubagentResult> {
    let task = self
      .tasks()
      .remove(name)
      .with_context(|| format!("no background subagent named {name} is waiting to be collected"))?;
    task.await.context("subagent task failed")?
  }

  pub fn running(&self) -> Vec<(String, bool)> {
    self
      .tasks()
      .iter()
      .map(|(name, task)| (name.clone(), task.is_finished()))
      .collect()
  }
}

pub fn subagent_tool(registry: Arc<SubagentRegistry>, mesh: bool) -> Arc<dyn Tool> {
  Arc::new(SubagentTool { registry, mesh })
}

struct SubagentTool {
  registry: Arc<SubagentRegistry>,
  mesh: bool,
}

#[derive(Deserialize)]
struct SubagentArgs {
  #[serde(default)]
  action: Option<String>,
  prompt: Option<String>,
  role: Option<String>,
  name: Option<String>,
  #[serde(default)]
  background: bool,
}

#[async_trait]
impl Tool for SubagentTool {
  fn spec(&self) -> ToolSpec {
    let mesh = if self.mesh {
      " Each one joins the Synapse mesh under its guardian name, so it can be messaged with \
       the mcp tool's send and waitstatus while it works."
    } else {
      ""
    };
    ToolSpec {
      name: "subagent".into(),
      description: format!(
        "Delegate a focused task to a durable child session. `delegate` runs one and returns \
         its answer; add background to start it and keep working, then `collect` it by name. \
         `list` shows what is still running.{mesh}"
      ),
      parameters: json!({
        "type": "object", "properties": {
          "action": {"type": "string", "enum": ["delegate", "collect", "list"]},
          "prompt": {"type": "string", "minLength": 1},
          "role": {"type": "string"},
          "name": {"type": "string"},
          "background": {"type": "boolean"}
        }, "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Execute
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let args: SubagentArgs = serde_json::from_value(arguments)?;
    let action = args.action.as_deref().unwrap_or("delegate");
    let output = match action {
      "delegate" => {
        let prompt = args.prompt.context("prompt is required")?;
        let request = SubagentRequest {
          parent_id: context.session_id,
          name: self.registry.next_name(),
          prompt,
          role: args.role,
        };
        if args.background {
          let name = self.registry.start(request);
          format!("{name} is working; collect it by name when you need the result")
        } else {
          serde_json::to_string_pretty(&self.registry.run(request).await?)?
        }
      }
      "collect" => {
        let name = args
          .name
          .context("name is required to collect a subagent")?;
        serde_json::to_string_pretty(&self.registry.collect(&name).await?)?
      }
      "list" => {
        let running = self.registry.running();
        if running.is_empty() {
          "no background subagents".to_string()
        } else {
          running
            .into_iter()
            .map(|(name, finished)| {
              format!("{name}  {}", if finished { "ready" } else { "working" })
            })
            .collect::<Vec<_>>()
            .join("\n")
        }
      }
      other => bail!("unknown subagent action: {other}"),
    };
    Ok(truncate(output, context.max_output_bytes))
  }
}
