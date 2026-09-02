use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::fs;

use crate::{
  frontmatter,
  mcp::{McpProfile, McpServerConfig, McpTransport, valid_name},
  memory::MemoryStore,
  prompt::PromptCatalog,
  skill::SkillCatalog,
};

const TIMEOUT_MS: u64 = 30_000;
const MAX_COPY_DEPTH: usize = 4;
const MAX_COPY_FILES: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportKind {
  Mcp,
  Skill,
  Prompt,
  Memory,
}

impl ImportKind {
  pub fn label(self) -> &'static str {
    match self {
      Self::Mcp => "tool servers",
      Self::Skill => "skills",
      Self::Prompt => "prompt templates",
      Self::Memory => "memories",
    }
  }

  pub fn parse(value: &str) -> Result<Self> {
    match value.trim() {
      "mcp" | "servers" => Ok(Self::Mcp),
      "skill" | "skills" => Ok(Self::Skill),
      "prompt" | "prompts" => Ok(Self::Prompt),
      "memory" | "memories" => Ok(Self::Memory),
      other => bail!("unknown import kind {other}; use mcp, skills, prompts, or memory"),
    }
  }
}

#[derive(Clone, Debug)]
enum Payload {
  Server(Box<McpServerConfig>),
  Directory(PathBuf),
  File(PathBuf),
  Text(String),
}

/// Something another tool already has that Ainz could take a copy of.
#[derive(Clone, Debug)]
pub struct Candidate {
  pub kind: ImportKind,
  pub name: String,
  pub origin: String,
  pub detail: String,
  /// The entry carries a token, key, or password inline rather than naming an environment
  /// variable, so importing copies the secret itself.
  pub secrets: bool,
  /// Ainz already has this, either from an earlier import or because it reads the source
  /// directly; importing it again would change nothing.
  pub present: bool,
  payload: Payload,
}

impl Candidate {
  pub fn target(&self) -> Result<String> {
    Ok(match self.kind {
      ImportKind::Mcp => McpProfile::path()?.display().to_string(),
      ImportKind::Skill => skills_root()?.join(&self.name).display().to_string(),
      ImportKind::Prompt => prompts_root()?
        .join(format!("{}.md", self.name))
        .display()
        .to_string(),
      ImportKind::Memory => "this workspace's memory".into(),
    })
  }
}

/// Everything importable that Ainz is not already reading.
pub async fn discover(workspace: &Path, memory: &MemoryStore) -> Result<Vec<Candidate>> {
  let mut candidates = Vec::new();
  candidates.extend(servers(workspace).await?);
  candidates.extend(skills(workspace).await?);
  candidates.extend(prompts(workspace).await?);
  candidates.extend(memories(workspace, memory).await?);
  candidates.sort_by(|left, right| {
    left
      .kind
      .cmp(&right.kind)
      .then(left.name.cmp(&right.name))
      .then(left.origin.cmp(&right.origin))
  });
  candidates.dedup_by(|left, right| left.kind == right.kind && left.name == right.name);
  Ok(candidates)
}

/// Copies the chosen entries into Ainz, returning one line each for what happened.
pub async fn import(chosen: &[Candidate], memory: &MemoryStore) -> Result<Vec<String>> {
  let mut done = Vec::new();
  let servers: Vec<&Candidate> = chosen
    .iter()
    .filter(|candidate| candidate.kind == ImportKind::Mcp)
    .collect();
  if !servers.is_empty() {
    let mut profile = McpProfile::load().await?;
    for candidate in &servers {
      let Payload::Server(config) = &candidate.payload else {
        continue;
      };
      profile
        .servers
        .insert(candidate.name.clone(), (**config).clone());
      done.push(format!("added tool server {}", candidate.name));
    }
    profile.validate()?;
    profile.save().await?;
  }

  for candidate in chosen {
    match (&candidate.kind, &candidate.payload) {
      (ImportKind::Skill, Payload::Directory(source)) => {
        let destination = skills_root()?.join(&candidate.name);
        copy_tree(source, &destination).await?;
        done.push(format!("copied skill {}", candidate.name));
      }
      (ImportKind::Prompt, Payload::File(source)) => {
        let root = prompts_root()?;
        fs::create_dir_all(&root).await?;
        let destination = root.join(format!("{}.md", candidate.name));
        fs::copy(source, &destination)
          .await
          .with_context(|| format!("copy {}", source.display()))?;
        done.push(format!("copied prompt {}", candidate.name));
      }
      (ImportKind::Memory, Payload::Text(body)) => {
        memory
          .remember(body, Some(&candidate.origin), "project", &[])
          .await?;
        done.push(format!("stored memory {}", candidate.name));
      }
      _ => {}
    }
  }
  Ok(done)
}

async fn servers(workspace: &Path) -> Result<Vec<Candidate>> {
  let mut found = Vec::new();
  let profile = McpProfile::load().await?;
  let home = dirs::home_dir();
  let mut sources: Vec<(String, PathBuf, Format)> = Vec::new();
  if let Some(home) = &home {
    sources.push((
      "Claude Code".into(),
      home.join(".claude.json"),
      Format::Claude(workspace.to_path_buf()),
    ));
    sources.push((
      "Claude Desktop".into(),
      home.join("Library/Application Support/Claude/claude_desktop_config.json"),
      Format::Json("mcpServers"),
    ));
    sources.push((
      "Codex".into(),
      home.join(".codex/config.toml"),
      Format::Codex,
    ));
    sources.push((
      "Cursor".into(),
      home.join(".cursor/mcp.json"),
      Format::Json("mcpServers"),
    ));
    sources.push((
      "Windsurf".into(),
      home.join(".codeium/windsurf/mcp_config.json"),
      Format::Json("mcpServers"),
    ));
    sources.push((
      "Gemini CLI".into(),
      home.join(".gemini/settings.json"),
      Format::Json("mcpServers"),
    ));
  }
  sources.push((
    "this workspace".into(),
    workspace.join(".mcp.json"),
    Format::Json("mcpServers"),
  ));
  sources.push((
    "this workspace".into(),
    workspace.join(".vscode/mcp.json"),
    Format::Json("servers"),
  ));
  sources.push((
    "this workspace".into(),
    workspace.join(".cursor/mcp.json"),
    Format::Json("mcpServers"),
  ));

  for (origin, path, format) in sources {
    let text = match fs::read_to_string(&path).await {
      Ok(text) => text,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(_) => continue,
    };
    for (name, config) in format.parse(&text)? {
      let name = sanitize(&name);
      if name.is_empty() {
        continue;
      }
      let detail = match (&config.url, config.command.first()) {
        (Some(url), _) => url.clone(),
        (None, Some(command)) => {
          let arguments = config.command[1..].join(" ");
          format!("{command} {arguments}").trim_end().to_string()
        }
        (None, None) => continue,
      };
      found.push(Candidate {
        kind: ImportKind::Mcp,
        secrets: carries_secrets(&config),
        present: profile.servers.contains_key(&name),
        name,
        origin: format!("{origin} · {}", short(&path)),
        detail,
        payload: Payload::Server(Box::new(config)),
      });
    }
  }
  Ok(found)
}

async fn skills(workspace: &Path) -> Result<Vec<Candidate>> {
  let known: Vec<String> = SkillCatalog::discover(workspace)
    .await?
    .skills
    .into_iter()
    .map(|skill| skill.name)
    .collect();
  let mut roots = Vec::new();
  if let Some(data) = dirs::data_dir() {
    roots.push(("Synapse library".to_string(), data.join("synapse/skills")));
  }
  // ~/.claude/skills and ~/.agents/skills are left out on purpose: Ainz already reads both
  if let Some(home) = dirs::home_dir() {
    roots.push(("Codex".to_string(), home.join(".codex/skills")));
    roots.push(("pi".to_string(), home.join(".pi/skills")));
  }

  let mut found = Vec::new();
  for (origin, root) in roots {
    let mut entries = match fs::read_dir(&root).await {
      Ok(entries) => entries,
      Err(_) => continue,
    };
    while let Some(entry) = entries.next_entry().await? {
      let manifest = entry.path().join("SKILL.md");
      let Ok(text) = fs::read_to_string(&manifest).await else {
        continue;
      };
      let front = frontmatter::parse(&text);
      let name = front
        .field("name")
        .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
      if name.trim().is_empty() {
        continue;
      }
      found.push(Candidate {
        kind: ImportKind::Skill,
        present: known.contains(&name),
        detail: front.field("description").unwrap_or_default(),
        origin: format!("{origin} · {}", short(&root)),
        secrets: false,
        name,
        payload: Payload::Directory(entry.path()),
      });
    }
  }
  Ok(found)
}

async fn prompts(workspace: &Path) -> Result<Vec<Candidate>> {
  let known: Vec<String> = PromptCatalog::discover(workspace)
    .await?
    .prompts
    .into_iter()
    .map(|prompt| prompt.name)
    .collect();
  let Some(home) = dirs::home_dir() else {
    return Ok(Vec::new());
  };
  let root = home.join(".codex/prompts");
  let mut entries = match fs::read_dir(&root).await {
    Ok(entries) => entries,
    Err(_) => return Ok(Vec::new()),
  };
  let mut found = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    let path = entry.path();
    if path.extension().is_none_or(|extension| extension != "md") {
      continue;
    }
    let Some(name) = path
      .file_stem()
      .map(|stem| stem.to_string_lossy().into_owned())
    else {
      continue;
    };
    let text = fs::read_to_string(&path).await.unwrap_or_default();
    let front = frontmatter::parse(&text);
    found.push(Candidate {
      kind: ImportKind::Prompt,
      present: known.contains(&name),
      detail: front
        .field("description")
        .unwrap_or_else(|| first_line(front.body)),
      origin: format!("Codex · {}", short(&root)),
      secrets: false,
      name,
      payload: Payload::File(path),
    });
  }
  Ok(found)
}

// Claude Code keeps a file per memory under a directory named for the workspace path
async fn memories(workspace: &Path, memory: &MemoryStore) -> Result<Vec<Candidate>> {
  if memory.is_off() {
    return Ok(Vec::new());
  }
  let Some(home) = dirs::home_dir() else {
    return Ok(Vec::new());
  };
  let slug: String = workspace
    .display()
    .to_string()
    .chars()
    .map(|ch| if ch == '/' || ch == '.' { '-' } else { ch })
    .collect();
  let root = home.join(".claude/projects").join(&slug).join("memory");
  let mut entries = match fs::read_dir(&root).await {
    Ok(entries) => entries,
    Err(_) => return Ok(Vec::new()),
  };
  let stored = memory.recall("", 200).await.unwrap_or_default();
  let mut found = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    let path = entry.path();
    if path.extension().is_none_or(|extension| extension != "md")
      || path.file_name().is_some_and(|name| name == "MEMORY.md")
    {
      continue;
    }
    let Ok(text) = fs::read_to_string(&path).await else {
      continue;
    };
    let front = frontmatter::parse(&text);
    let body = front.body.trim().to_string();
    if body.is_empty() {
      continue;
    }
    let name = front.field("name").unwrap_or_else(|| {
      path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into()
    });
    found.push(Candidate {
      kind: ImportKind::Memory,
      present: stored.iter().any(|record| record.body == body),
      detail: front
        .field("description")
        .unwrap_or_else(|| first_line(&body)),
      origin: format!("Claude Code · {}", short(&root)),
      secrets: false,
      name,
      payload: Payload::Text(body),
    });
  }
  Ok(found)
}

enum Format {
  /// `~/.claude.json`, which keeps global servers beside a map of per-project ones
  Claude(PathBuf),
  Json(&'static str),
  Codex,
}

impl Format {
  fn parse(&self, text: &str) -> Result<Vec<(String, McpServerConfig)>> {
    match self {
      Self::Claude(workspace) => {
        let file: ClaudeFile = match serde_json::from_str(text) {
          Ok(file) => file,
          Err(_) => return Ok(Vec::new()),
        };
        let mut servers: Vec<_> = file
          .mcp_servers
          .into_iter()
          .map(|(name, server)| (name, server.into()))
          .collect();
        // Claude Code stores the path it was launched with; Ainz canonicalizes its
        // workspace, and on macOS those differ by /private
        let target = canonical(workspace);
        for (path, project) in &file.projects {
          if canonical(path) != target {
            continue;
          }
          servers.extend(
            project
              .mcp_servers
              .clone()
              .into_iter()
              .map(|(name, server)| (name, server.into())),
          );
        }
        Ok(servers)
      }
      Self::Json(key) => {
        let value: serde_json::Value = match serde_json::from_str(text) {
          Ok(value) => value,
          Err(_) => return Ok(Vec::new()),
        };
        let Some(map) = value.get(key).and_then(|entry| entry.as_object()) else {
          return Ok(Vec::new());
        };
        Ok(
          map
            .iter()
            .filter_map(|(name, entry)| {
              serde_json::from_value::<ExternalServer>(entry.clone())
                .ok()
                .map(|server| (name.clone(), server.into()))
            })
            .collect(),
        )
      }
      Self::Codex => {
        let file: CodexFile = match toml::from_str(text) {
          Ok(file) => file,
          Err(_) => return Ok(Vec::new()),
        };
        Ok(
          file
            .mcp_servers
            .into_iter()
            .map(|(name, server)| (name, server.into()))
            .collect(),
        )
      }
    }
  }
}

#[derive(Deserialize)]
struct ClaudeFile {
  #[serde(default, rename = "mcpServers")]
  mcp_servers: BTreeMap<String, ExternalServer>,
  #[serde(default)]
  projects: BTreeMap<PathBuf, ClaudeProject>,
}

#[derive(Clone, Deserialize)]
struct ClaudeProject {
  #[serde(default, rename = "mcpServers")]
  mcp_servers: BTreeMap<String, ExternalServer>,
}

#[derive(Clone, Deserialize)]
struct ExternalServer {
  #[serde(default)]
  command: Option<String>,
  #[serde(default)]
  args: Vec<String>,
  #[serde(default)]
  env: BTreeMap<String, String>,
  #[serde(default)]
  url: Option<String>,
  #[serde(default)]
  headers: BTreeMap<String, String>,
  #[serde(default)]
  cwd: Option<PathBuf>,
  #[serde(default)]
  disabled: Option<bool>,
}

impl From<ExternalServer> for McpServerConfig {
  fn from(server: ExternalServer) -> Self {
    server_config(
      server.command,
      server.args,
      server.env,
      server.url,
      server.headers,
      server.cwd,
      server.disabled.map(|disabled| !disabled),
    )
  }
}

#[derive(Deserialize)]
struct CodexFile {
  #[serde(default)]
  mcp_servers: BTreeMap<String, CodexServer>,
}

#[derive(Deserialize)]
struct CodexServer {
  #[serde(default)]
  command: Option<String>,
  #[serde(default)]
  args: Vec<String>,
  #[serde(default)]
  env: BTreeMap<String, String>,
  #[serde(default)]
  url: Option<String>,
  #[serde(default)]
  http_headers: BTreeMap<String, String>,
  #[serde(default)]
  cwd: Option<PathBuf>,
  #[serde(default)]
  enabled: Option<bool>,
}

impl From<CodexServer> for McpServerConfig {
  fn from(server: CodexServer) -> Self {
    server_config(
      server.command,
      server.args,
      server.env,
      server.url,
      server.http_headers,
      server.cwd,
      server.enabled,
    )
  }
}

fn server_config(
  command: Option<String>,
  args: Vec<String>,
  env: BTreeMap<String, String>,
  url: Option<String>,
  headers: BTreeMap<String, String>,
  cwd: Option<PathBuf>,
  enabled: Option<bool>,
) -> McpServerConfig {
  // other harnesses distinguish sse from streamable http; Ainz speaks one HTTP transport
  let transport = if command.is_some() {
    McpTransport::Stdio
  } else {
    McpTransport::StreamableHttp
  };
  McpServerConfig {
    transport,
    command: command.into_iter().chain(args).collect(),
    url,
    header_env: BTreeMap::new(),
    headers,
    env,
    cwd,
    enabled: enabled.unwrap_or(true),
    // an imported server is never required: a broken one should not stop a session starting
    required: false,
    timeout_ms: TIMEOUT_MS,
  }
}

fn carries_secrets(config: &McpServerConfig) -> bool {
  const MARKERS: [&str; 6] = ["token", "key", "secret", "password", "credential", "auth"];
  let named = |name: &str| {
    let name = name.to_lowercase();
    MARKERS.iter().any(|marker| name.contains(marker))
  };
  !config.headers.is_empty() || config.env.keys().any(|key| named(key))
}

fn sanitize(name: &str) -> String {
  let cleaned: String = name
    .chars()
    .map(|ch| {
      if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
        ch
      } else {
        '-'
      }
    })
    .collect();
  if valid_name(&cleaned) {
    cleaned
  } else {
    cleaned.chars().take(64).collect()
  }
}

fn canonical(path: &Path) -> PathBuf {
  std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn short(path: &Path) -> String {
  match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
    Some(relative) => format!("~/{}", relative.display()),
    None => path.display().to_string(),
  }
}

fn first_line(text: &str) -> String {
  text
    .lines()
    .find(|line| !line.trim().is_empty())
    .unwrap_or_default()
    .trim()
    .to_string()
}

fn skills_root() -> Result<PathBuf> {
  Ok(
    dirs::config_dir()
      .context("could not locate the config directory")?
      .join("ainz/skills"),
  )
}

fn prompts_root() -> Result<PathBuf> {
  Ok(
    dirs::config_dir()
      .context("could not locate the config directory")?
      .join("ainz/prompts"),
  )
}

// a skill is its directory: the manifest plus whatever scripts and references sit beside it
async fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
  let mut pending = vec![(source.to_path_buf(), destination.to_path_buf(), 0_usize)];
  let mut files = 0;
  while let Some((from, to, depth)) = pending.pop() {
    fs::create_dir_all(&to)
      .await
      .with_context(|| format!("create {}", to.display()))?;
    let mut entries = fs::read_dir(&from)
      .await
      .with_context(|| format!("read {}", from.display()))?;
    while let Some(entry) = entries.next_entry().await? {
      let Ok(file_type) = entry.file_type().await else {
        continue;
      };
      let target = to.join(entry.file_name());
      if file_type.is_dir() {
        if depth + 1 < MAX_COPY_DEPTH {
          pending.push((entry.path(), target, depth + 1));
        }
        continue;
      }
      if !file_type.is_file() {
        continue;
      }
      files += 1;
      if files > MAX_COPY_FILES {
        bail!(
          "{} holds more than {MAX_COPY_FILES} files",
          source.display()
        );
      }
      fs::copy(entry.path(), &target)
        .await
        .with_context(|| format!("copy {}", entry.path().display()))?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn claude_projects_contribute_their_own_servers() {
    let text = r#"{
      "mcpServers": {"files": {"command": "server", "args": ["--stdio"]}},
      "projects": {"/work": {"mcpServers": {"local": {"command": "other"}}}}
    }"#;
    let parsed = Format::Claude(PathBuf::from("/work")).parse(text).unwrap();
    let names: Vec<_> = parsed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["files", "local"]);
    assert_eq!(parsed[0].1.command, ["server", "--stdio"]);
    assert_eq!(parsed[0].1.transport, McpTransport::Stdio);
    // an imported server never blocks startup
    assert!(!parsed[0].1.required);
  }

  #[test]
  fn a_project_that_is_not_this_one_is_left_alone() {
    let text = r#"{"projects": {"/elsewhere": {"mcpServers": {"nope": {"command": "x"}}}}}"#;
    assert!(
      Format::Claude(PathBuf::from("/work"))
        .parse(text)
        .unwrap()
        .is_empty()
    );
  }

  #[test]
  fn codex_tables_become_servers_and_keep_their_headers() {
    let text = r#"
[mcp_servers.github]
url = "https://api.example/mcp/"

[mcp_servers.github.http_headers]
Authorization = "Bearer secret"

[mcp_servers.local]
command = "run"
args = ["mcp"]
enabled = false

[mcp_servers.local.tools.read]
approval_mode = "approve"
"#;
    let parsed = Format::Codex.parse(text).unwrap();
    let servers: BTreeMap<_, _> = parsed.into_iter().collect();
    assert_eq!(servers["github"].transport, McpTransport::StreamableHttp);
    assert!(carries_secrets(&servers["github"]));
    assert!(!servers["local"].enabled);
    assert!(!carries_secrets(&servers["local"]));
  }

  #[test]
  fn names_other_tools_allow_are_made_safe() {
    assert_eq!(sanitize("my server!"), "my-server-");
    assert_eq!(sanitize("files"), "files");
  }
}
