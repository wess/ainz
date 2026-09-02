use std::{
  collections::HashMap,
  path::PathBuf,
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use tokio::fs;
use uuid::Uuid;

use crate::protocol::{Message, Role, Usage};

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

fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
