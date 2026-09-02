use std::{
  collections::HashMap,
  path::PathBuf,
  sync::Arc,
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::{Value, json};
use tokio::fs;
use uuid::Uuid;

use crate::{
  protocol::{Message, Role, ToolSpec, Usage},
  tool::{Risk, Tool, ToolContext, truncate},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionNode {
  pub id: Uuid,
  pub parent: Option<Uuid>,
  pub created_at: u64,
  pub message: Message,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
  pub cursor: Uuid,
  pub created_at: u64,
  pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
  pub id: Uuid,
  #[serde(default)]
  pub parent_id: Option<Uuid>,
  pub workspace: PathBuf,
  pub created_at: u64,
  pub updated_at: u64,
  pub cursor: Option<Uuid>,
  pub nodes: Vec<SessionNode>,
  #[serde(default)]
  pub summaries: Vec<SessionSummary>,
  #[serde(default)]
  pub usage: Usage,
}

impl Session {
  pub fn new(workspace: PathBuf) -> Self {
    let now = now();
    Self {
      id: Uuid::now_v7(),
      parent_id: None,
      workspace,
      created_at: now,
      updated_at: now,
      cursor: None,
      nodes: Vec::new(),
      summaries: Vec::new(),
      usage: Usage::default(),
    }
  }

  pub fn child(workspace: PathBuf, parent_id: Uuid) -> Self {
    let mut session = Self::new(workspace);
    session.parent_id = Some(parent_id);
    session
  }

  pub fn append(&mut self, message: Message) -> Uuid {
    let id = Uuid::now_v7();
    let created_at = now();
    self.nodes.push(SessionNode {
      id,
      parent: self.cursor,
      created_at,
      message,
    });
    self.cursor = Some(id);
    self.updated_at = created_at;
    id
  }

  pub fn checkout(&mut self, cursor: Option<Uuid>) -> Result<()> {
    if let Some(id) = cursor
      && !self.nodes.iter().any(|node| node.id == id)
    {
      bail!("session node {id} does not exist");
    }
    self.cursor = cursor;
    Ok(())
  }

  pub fn messages(&self) -> Result<Vec<Message>> {
    Ok(
      self
        .active_nodes()?
        .into_iter()
        .map(|node| node.message.clone())
        .collect(),
    )
  }

  pub fn context_messages(&self) -> Result<Vec<Message>> {
    let nodes = self.active_nodes()?;
    let summary = self.active_summary(&nodes);
    let mut messages = Vec::new();
    let start = if let Some(summary) = summary {
      messages.push(Message::text(
        Role::System,
        format!("Summary of earlier session context:\n{}", summary.summary),
      ));
      nodes
        .iter()
        .position(|node| node.id == summary.cursor)
        .map_or(0, |index| index + 1)
    } else {
      0
    };
    messages.extend(nodes[start..].iter().map(|node| node.message.clone()));
    Ok(messages)
  }

  pub fn compaction_input(&self, preserve: usize) -> Result<Option<(Uuid, Vec<Message>, usize)>> {
    let nodes = self.active_nodes()?;
    let summary = self.active_summary(&nodes);
    let start = summary
      .and_then(|summary| {
        nodes
          .iter()
          .position(|node| node.id == summary.cursor)
          .map(|index| index + 1)
      })
      .unwrap_or(0);
    if nodes.len().saturating_sub(start) <= preserve {
      return Ok(None);
    }

    let mut keep = nodes.len() - preserve;
    while keep > start && nodes[keep].message.role != Role::User {
      keep -= 1;
    }
    if keep == start {
      return Ok(None);
    }
    let cursor = nodes[keep - 1].id;
    let mut messages = Vec::new();
    if let Some(summary) = summary {
      messages.push(Message::text(Role::System, &summary.summary));
    }
    messages.extend(nodes[start..keep].iter().map(|node| node.message.clone()));
    Ok(Some((cursor, messages, keep - start)))
  }

  pub fn record_summary(&mut self, cursor: Uuid, summary: String) -> Result<()> {
    if !self.active_nodes()?.iter().any(|node| node.id == cursor) {
      bail!("cannot compact at inactive session node {cursor}");
    }
    self.summaries.push(SessionSummary {
      cursor,
      created_at: now(),
      summary,
    });
    Ok(())
  }

  fn active_nodes(&self) -> Result<Vec<&SessionNode>> {
    let nodes: HashMap<_, _> = self.nodes.iter().map(|node| (node.id, node)).collect();
    let mut cursor = self.cursor;
    let mut active = Vec::new();
    while let Some(id) = cursor {
      let node = nodes
        .get(&id)
        .with_context(|| format!("missing session node {id}"))?;
      active.push(*node);
      cursor = node.parent;
    }
    active.reverse();
    Ok(active)
  }

  fn active_summary<'a>(&'a self, nodes: &[&SessionNode]) -> Option<&'a SessionSummary> {
    self
      .summaries
      .iter()
      .rev()
      .find(|summary| nodes.iter().any(|node| node.id == summary.cursor))
  }
}

// what a listing needs, read without materializing every message or attachment
#[derive(Clone, Debug, Serialize)]
pub struct SessionInfo {
  pub id: Uuid,
  pub parent_id: Option<Uuid>,
  pub workspace: PathBuf,
  pub created_at: u64,
  pub updated_at: u64,
  pub usage: Usage,
  pub nodes: usize,
}

#[derive(Deserialize)]
struct SessionHead {
  id: Uuid,
  #[serde(default)]
  parent_id: Option<Uuid>,
  workspace: PathBuf,
  created_at: u64,
  updated_at: u64,
  #[serde(default)]
  usage: Usage,
  #[serde(default)]
  nodes: Vec<IgnoredAny>,
}

impl From<SessionHead> for SessionInfo {
  fn from(head: SessionHead) -> Self {
    Self {
      id: head.id,
      parent_id: head.parent_id,
      workspace: head.workspace,
      created_at: head.created_at,
      updated_at: head.updated_at,
      usage: head.usage,
      nodes: head.nodes.len(),
    }
  }
}

#[derive(Clone, Debug)]
pub struct SessionStore {
  root: PathBuf,
  legacy_root: Option<PathBuf>,
}

impl SessionStore {
  pub fn new(root: PathBuf) -> Self {
    Self {
      root,
      legacy_root: None,
    }
  }

  pub fn default_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("could not locate the data directory")?;
    Ok(base.join("ainz/sessions"))
  }

  pub fn default_store() -> Result<Self> {
    let base = dirs::data_local_dir().context("could not locate the data directory")?;
    Ok(Self {
      root: Self::default_path()?,
      legacy_root: Some(base.join("agentx/sessions")),
    })
  }

  pub async fn save(&self, session: &Session) -> Result<()> {
    fs::create_dir_all(&self.root).await?;
    let path = self.path(session.id);
    let temp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(session)?;
    fs::write(&temp, data).await?;
    fs::rename(&temp, &path).await?;
    Ok(())
  }

  pub async fn load(&self, id: Uuid) -> Result<Session> {
    let path = self.path(id);
    let path = if path.exists() {
      path
    } else {
      self
        .legacy_root
        .as_ref()
        .map(|root| root.join(format!("{id}.json")))
        .unwrap_or(path)
    };
    let data = fs::read(&path)
      .await
      .with_context(|| format!("read session {id}"))?;
    serde_json::from_slice(&data).with_context(|| format!("parse session {id}"))
  }

  pub async fn list(&self) -> Result<Vec<SessionInfo>> {
    let mut sessions: HashMap<Uuid, SessionInfo> = HashMap::new();
    let roots = self.legacy_root.iter().chain(std::iter::once(&self.root));
    for root in roots {
      let mut entries = match fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
        Err(error) => return Err(error).context("read session directory"),
      };
      while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().is_some_and(|ext| ext == "json")
          && let Ok(data) = fs::read(entry.path()).await
          && let Ok(head) = serde_json::from_slice::<SessionHead>(&data)
        {
          sessions.insert(head.id, head.into());
        }
      }
    }
    let mut sessions: Vec<_> = sessions.into_values().collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
  }

  fn path(&self, id: Uuid) -> PathBuf {
    self.root.join(format!("{id}.json"))
  }
}

// what a search reads: message text and summaries, without images or tool arguments
#[derive(Deserialize)]
struct SearchDocument {
  id: Uuid,
  workspace: PathBuf,
  created_at: u64,
  updated_at: u64,
  #[serde(default)]
  nodes: Vec<SearchNode>,
  #[serde(default)]
  summaries: Vec<SessionSummary>,
}

#[derive(Deserialize)]
struct SearchNode {
  message: SearchMessage,
}

#[derive(Deserialize)]
struct SearchMessage {
  role: Role,
  #[serde(default)]
  content: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionMatch {
  pub id: Uuid,
  pub workspace: PathBuf,
  pub created_at: u64,
  pub updated_at: u64,
  pub score: usize,
  pub excerpts: Vec<String>,
}

const MAX_EXCERPTS: usize = 3;
const EXCERPT_CHARS: usize = 240;

impl SessionStore {
  /// Full-text search across stored sessions. Terms are matched case-insensitively; a session
  /// scores once per distinct term it contains, so the closest transcripts sort first.
  pub async fn search(
    &self,
    query: &str,
    workspace: Option<&std::path::Path>,
    limit: usize,
  ) -> Result<Vec<SessionMatch>> {
    let terms: Vec<String> = query
      .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '.' && ch != '/')
      .filter(|term| !term.is_empty())
      .map(str::to_lowercase)
      .collect();
    if terms.is_empty() {
      bail!("a session search needs at least one term");
    }
    let mut found: HashMap<Uuid, SessionMatch> = HashMap::new();
    let roots = self.legacy_root.iter().chain(std::iter::once(&self.root));
    for root in roots {
      let mut entries = match fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
        Err(error) => return Err(error).context("read session directory"),
      };
      while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().is_none_or(|ext| ext != "json") {
          continue;
        }
        let Ok(data) = fs::read(entry.path()).await else {
          continue;
        };
        let Ok(document) = serde_json::from_slice::<SearchDocument>(&data) else {
          continue;
        };
        if workspace.is_some_and(|path| document.workspace != path) {
          continue;
        }
        if let Some(hit) = search_document(&document, &terms) {
          found.insert(hit.id, hit);
        }
      }
    }
    let mut matches: Vec<_> = found.into_values().collect();
    matches.sort_by(|left, right| {
      right
        .score
        .cmp(&left.score)
        .then(right.updated_at.cmp(&left.updated_at))
    });
    matches.truncate(limit);
    Ok(matches)
  }

  pub fn tool(&self) -> Arc<dyn Tool> {
    Arc::new(SessionTool {
      store: self.clone(),
    })
  }
}

fn search_document(document: &SearchDocument, terms: &[String]) -> Option<SessionMatch> {
  let texts: Vec<(&'static str, &str)> = document
    .summaries
    .iter()
    .map(|summary| ("summary", summary.summary.as_str()))
    .chain(document.nodes.iter().filter_map(|node| {
      let label = match node.message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
      };
      node.message.content.as_deref().map(|text| (label, text))
    }))
    .collect();
  let mut score = 0;
  let mut excerpts = Vec::new();
  for term in terms {
    let mut hit = false;
    for (label, text) in &texts {
      let lower = text.to_lowercase();
      let Some(position) = lower.find(term.as_str()) else {
        continue;
      };
      hit = true;
      if excerpts.len() < MAX_EXCERPTS {
        excerpts.push(format!("{label}: {}", excerpt(text, position)));
      }
      break;
    }
    if hit {
      score += 1;
    }
  }
  (score > 0).then(|| SessionMatch {
    id: document.id,
    workspace: document.workspace.clone(),
    created_at: document.created_at,
    updated_at: document.updated_at,
    score,
    excerpts,
  })
}

fn excerpt(text: &str, position: usize) -> String {
  let start = text[..position]
    .char_indices()
    .rev()
    .nth(EXCERPT_CHARS / 3)
    .map_or(0, |(index, _)| index);
  let window: String = text[start..]
    .chars()
    .take(EXCERPT_CHARS)
    .collect::<String>()
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ");
  if start == 0 {
    window
  } else {
    format!("…{window}")
  }
}

struct SessionTool {
  store: SessionStore,
}

#[derive(Deserialize)]
struct SessionArgs {
  query: String,
  #[serde(default)]
  limit: Option<usize>,
  #[serde(default)]
  all_workspaces: bool,
}

#[async_trait]
impl Tool for SessionTool {
  fn spec(&self) -> ToolSpec {
    ToolSpec {
      name: "sessions".into(),
      description: "Search earlier sessions in this workspace for what was said, tried, or \
                    decided. Use it before re-deriving something that was worked out before, \
                    such as an error seen last week or a command that worked. Returns session \
                    ids with excerpts; `ainz resume ID` opens one."
        .into(),
      parameters: json!({
        "type": "object", "properties": {
          "query": {"type": "string", "minLength": 2},
          "limit": {"type": "integer", "minimum": 1, "maximum": 25},
          "all_workspaces": {"type": "boolean"}
        }, "required": ["query"], "additionalProperties": false
      }),
    }
  }

  fn risk(&self, _arguments: &Value) -> Risk {
    Risk::Read
  }

  async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
    let args: SessionArgs = serde_json::from_value(arguments)?;
    let workspace = (!args.all_workspaces).then_some(context.workspace.as_path());
    let matches = self
      .store
      .search(&args.query, workspace, args.limit.unwrap_or(5).clamp(1, 25))
      .await?;
    if matches.is_empty() {
      return Ok("no earlier session mentioned that".into());
    }
    Ok(truncate(
      serde_json::to_string_pretty(&matches)?,
      context.max_output_bytes,
    ))
  }
}

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
