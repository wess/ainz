#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
  pub name: String,
  pub usage: String,
  pub description: String,
  pub source: String,
}

impl SlashCommand {
  pub fn new(
    name: impl Into<String>,
    usage: impl Into<String>,
    description: impl Into<String>,
    source: impl Into<String>,
  ) -> Self {
    Self {
      name: name.into(),
      usage: usage.into(),
      description: description.into(),
      source: source.into(),
    }
  }

  pub fn completion(&self) -> String {
    if self.usage.contains('<') || self.usage.contains('[') {
      format!("/{} ", self.name)
    } else {
      format!("/{}", self.name)
    }
  }
}

pub fn builtins() -> Vec<SlashCommand> {
  [
    (
      "agent",
      "/agent <N>",
      "Switch to an agent transcript",
      "session",
    ),
    ("agents", "/agents", "Show the subagent roster", "view"),
    ("cancel", "/cancel", "Cancel the active run", "run"),
    (
      "checkout",
      "/checkout <NODE>",
      "Move to a session history node",
      "session",
    ),
    (
      "clear",
      "/clear",
      "Clear the transcript and start a new session",
      "session",
    ),
    (
      "config",
      "/config",
      "Configure providers and models",
      "config",
    ),
    ("exit", "/exit", "Save and exit Ainz", "session"),
    (
      "help",
      "/help",
      "Show commands and keyboard shortcuts",
      "help",
    ),
    (
      "header",
      "/header <NAME|random|builtin>",
      "Choose and remember the empty-screen header",
      "view",
    ),
    ("headers", "/headers", "List custom header artwork", "view"),
    (
      "history",
      "/history",
      "List nodes in the current session",
      "session",
    ),
    (
      "import",
      "/import",
      "Import tool servers, skills, and prompts from your other tools",
      "extension",
    ),
    (
      "image",
      "/image <PATH> <PROMPT>",
      "Attach an image to a prompt",
      "prompt",
    ),
    ("mcp", "/mcp", "List configured MCP servers", "extension"),
    (
      "memory",
      "/memory [QUERY]",
      "Recall what has been written down for this workspace",
      "memory",
    ),
    (
      "model",
      "/model",
      "Choose the active provider and model",
      "config",
    ),
    ("new", "/new", "Start a new session", "session"),
    (
      "permissions",
      "/permissions [MODE]",
      "Show or set ask, auto, or read-only mode",
      "config",
    ),
    (
      "plugins",
      "/plugins",
      "List discovered plugins and approval state",
      "extension",
    ),
    (
      "prompts",
      "/prompts",
      "List discovered prompt templates",
      "extension",
    ),
    (
      "provider",
      "/provider",
      "Configure or switch providers",
      "config",
    ),
    ("quit", "/quit", "Save and exit Ainz", "session"),
    (
      "remember",
      "/remember <TEXT>",
      "Store a durable fact, decision, or correction",
      "memory",
    ),
    (
      "sessions",
      "/sessions",
      "List saved sessions for this workspace",
      "session",
    ),
    (
      "settings",
      "/settings",
      "Open settings: memory, Synapse, permissions, and more",
      "config",
    ),
    ("skills", "/skills", "List discovered skills", "extension"),
    (
      "status",
      "/status",
      "Show session, model, permissions, and agents",
      "info",
    ),
    (
      "synapse",
      "/synapse",
      "Show the Synapse integration state",
      "config",
    ),
    (
      "yeet",
      "/yeet",
      "Run wide open: allow every tool call without asking",
      "config",
    ),
    (
      "usage",
      "/usage",
      "Show token usage for the selected agent",
      "info",
    ),
  ]
  .into_iter()
  .map(|(name, usage, description, source)| SlashCommand::new(name, usage, description, source))
  .collect()
}

pub fn matches<'a>(commands: &'a [SlashCommand], input: &str) -> Vec<&'a SlashCommand> {
  let Some(query) = input.strip_prefix('/') else {
    return Vec::new();
  };
  if query.contains(char::is_whitespace) {
    return Vec::new();
  }
  let query = query.to_ascii_lowercase();
  let mut ranked: Vec<_> = commands
    .iter()
    .filter_map(|command| score(command, &query).map(|score| (score, command)))
    .collect();
  ranked.sort_by(|(left_score, left), (right_score, right)| {
    left_score
      .cmp(right_score)
      .then_with(|| left.name.cmp(&right.name))
  });
  ranked.into_iter().map(|(_, command)| command).collect()
}

fn score(command: &SlashCommand, query: &str) -> Option<usize> {
  if query.is_empty() {
    return Some(100);
  }
  let name = command.name.to_ascii_lowercase();
  if name == query {
    return Some(0);
  }
  if name.starts_with(query) {
    return Some(10 + name.len() - query.len());
  }
  if let Some(index) = name.find(query) {
    return Some(30 + index);
  }
  if is_subsequence(query, &name) {
    return Some(50 + name.len());
  }
  let haystack = format!("{} {}", command.usage, command.description).to_ascii_lowercase();
  haystack.find(query).map(|index| 80 + index)
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
  let mut chars = needle.chars();
  let mut next = chars.next();
  for candidate in haystack.chars() {
    if next == Some(candidate) {
      next = chars.next();
      if next.is_none() {
        return true;
      }
    }
  }
  next.is_none()
}
