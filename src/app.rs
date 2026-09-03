use std::{
  io::{IsTerminal, Write},
  path::PathBuf,
  sync::Arc,
};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};

use ainz::{
  Agent, Approver, Config, Event, EventSink, HttpProvider, JobStore, McpHub, McpProfile,
  PermissionMode, PluginCatalog, ProcessOutput, ProcessProvider, PromptCatalog, ProviderConfig,
  ProviderKind, RunOptions, RuntimeProvider, Session, SessionStore, SkillCatalog, SubagentHandler,
  SubagentRegistry, SubagentRequest, SubagentResult,
  agent::Approval,
  config::MemoryBackend,
  deny_all, instruction,
  learn::Teacher,
  memory::{MemoryStore, recalled_section},
  protocol::{Image, ToolCall},
  run_control, subagent_tool, synapse,
  synapse::Synapse,
  tool::{Risk, ToolSet, builtins},
};

use crate::command::{ProviderPreset, preset_profile};

async fn make_agent(
  workspace: &std::path::Path,
  config: &Config,
  json: bool,
) -> Result<(Agent<RuntimeProvider>, RunOptions)> {
  let events = output_events(json);
  let approver = if json { deny_all() } else { Arc::new(approve) };
  make_agent_with(workspace, config, events, approver).await
}

/// The memory store on its own, for surfaces that want to recall or remember without
/// building a whole agent.
pub(crate) async fn memory_store(
  workspace: &std::path::Path,
  config: &Config,
) -> Result<MemoryStore> {
  match config.memory.backend {
    MemoryBackend::Off => Ok(MemoryStore::Off),
    MemoryBackend::Local => MemoryStore::local(workspace),
    MemoryBackend::Synapse => match synapse::binary(&config.synapse) {
      Some(binary) => {
        let mut profile = McpProfile::default();
        profile.servers.insert(
          synapse::SERVER.to_string(),
          synapse::server_config(&binary, workspace),
        );
        Ok(MemoryStore::Synapse(Synapse::new(
          Arc::new(McpHub::new(profile)),
          workspace.to_path_buf(),
        )))
      }
      None => MemoryStore::local(workspace),
    },
  }
}

pub(crate) async fn make_agent_with(
  workspace: &std::path::Path,
  config: &Config,
  events: EventSink,
  approver: Approver,
) -> Result<(Agent<RuntimeProvider>, RunOptions)> {
  let profile = config.active_provider()?;
  let provider = match profile.kind {
    ProviderKind::Http => RuntimeProvider::Http(HttpProvider::new(
      profile
        .endpoint
        .clone()
        .context("HTTP provider requires an endpoint")?,
      config.model.clone(),
      config.api_key_for(&profile)?,
    )?),
    ProviderKind::Process => RuntimeProvider::Process(ProcessProvider::new(
      profile
        .command
        .context("process provider requires a command")?,
      profile.args,
      config.model.clone(),
      workspace.to_path_buf(),
      config.permissions,
      profile.output,
    )),
  };
  let mut catalog = PluginCatalog::discover(workspace).await?;
  if config.yeet {
    catalog.trust_all();
  }
  let mut tools = ToolSet::default();
  tools.extend(builtins())?;
  tools.insert(JobStore::default_store()?.tool())?;
  tools.insert(SessionStore::default_store()?.tool())?;
  tools.insert(
    SkillCatalog::discover_with_roots(workspace, &catalog.approved_skill_roots())
      .await?
      .tool(),
  )?;
  let mut mcp_profile = catalog
    .merge_mcp(McpProfile::load_with(config.mcp_config.as_deref()).await?)
    .await?;
  // Synapse is optional in both directions: the setting has to be on and the binary has to exist
  let synapse_binary = config
    .synapse_active()
    .then(|| synapse::binary(&config.synapse))
    .flatten();
  if let Some(binary) = &synapse_binary {
    mcp_profile
      .servers
      .entry(synapse::SERVER.to_string())
      .or_insert_with(|| synapse::server_config(binary, workspace));
  }
  let mcp = Arc::new(McpHub::new(mcp_profile.clone()));
  if !mcp.is_empty() {
    mcp.ready().await?;
  }
  tools.extend(catalog.approved_tools().await?)?;
  let mut instructions = instruction::load(workspace).await?;
  for (name, text) in mcp.instructions().await? {
    instructions.push_str(&format!(
      "\n\nInstructions from MCP server {name}:\n{}",
      text.trim()
    ));
  }
  if !mcp.is_empty() {
    tools.insert(mcp.clone().tool())?;
  }
  let synapse = synapse_binary
    .is_some()
    .then(|| Synapse::new(mcp.clone(), workspace.to_path_buf()));
  // SOUL.md and whatever else Synapse tells its clients, which no other server here provides
  if let Some(synapse) = &synapse
    && let Some(guidance) = synapse.guidance().await
    && !guidance.trim().is_empty()
  {
    instructions.push_str(&format!("\n\nGuidance from Synapse:\n{}", guidance.trim()));
  }
  let memory = match config.memory.backend {
    MemoryBackend::Off => MemoryStore::Off,
    MemoryBackend::Local => MemoryStore::local(workspace)?,
    // asking for Synapse and quietly getting something else is worse than saying so
    MemoryBackend::Synapse => match synapse.clone() {
      Some(synapse) => MemoryStore::Synapse(synapse),
      None => {
        events.emit(Event::Error {
          message: format!(
            "memory is set to synapse but no synapse binary was found; using local memory \
             for this session — {}",
            synapse::SITE
          ),
        });
        MemoryStore::local(workspace)?
      }
    },
  };
  if !memory.is_off() {
    tools.insert(memory.tool())?;
    if config.memory.recall_on_start {
      // no query: what a session opens with is the newest of what this workspace stored
      match memory.recall("", config.memory.recall_limit).await {
        Ok(records) if !records.is_empty() => {
          instructions.push_str("\n\n");
          instructions.push_str(&recalled_section(&records));
        }
        Ok(_) => {}
        Err(error) => events.emit(Event::Error {
          message: format!("memory recall failed: {error:#}"),
        }),
      }
    }
  }
  if config.memory.teach && !memory.is_off() {
    let teacher = match &memory {
      MemoryStore::Synapse(synapse) => Teacher::Synapse(synapse.clone()),
      _ => Teacher::local()?,
    };
    tools.insert(teacher.tool())?;
  }
  // a lead on the mesh is addressable by name, so a worker can ask it something directly
  if config.mesh_active()
    && let Some(synapse) = &synapse
  {
    let name = mesh_name(workspace);
    let role = format!("ainz session in {}", workspace.display());
    match synapse.register(&name, &role).await {
      Ok(roster) => instructions.push_str(&format!(
        "\n\nYou are on the Synapse agent mesh as `{name}`. Reach other agents with the mcp \
         tool: synapse/agents lists who is here, synapse/send messages one, synapse/post \
         writes to a channel, synapse/waitstatus blocks until one reaches a state, and \
         synapse/reportstatus tells the person watching what you are doing. Subagents you \
         delegate to join under their own guardian names.\n{}",
        roster.trim()
      )),
      Err(error) => events.emit(Event::Error {
        message: format!("could not join the Synapse mesh: {error:#}"),
      }),
    }
  }
  let options = RunOptions {
    instructions,
    permissions: config.permissions,
    rules: config.rules.clone(),
    max_steps: config.max_steps,
    max_output_bytes: config.max_output_bytes,
    context_tokens: config.context_tokens,
    compact_at_tokens: config.compact_at_tokens,
    preserve_messages: config.preserve_messages,
    memory_nudge: (config.memory.remember_on_compact && !memory.is_off()).then(|| {
      "The transcript above was just compacted. If anything you worked out in it is durable \
       and is not written down yet, call memory remember now, then carry on."
        .to_string()
    }),
  };
  let child_provider = provider.clone();
  let child_tools = tools.clone();
  let child_workspace = workspace.to_path_buf();
  let child_options = options.clone();
  let child_approver = approver.clone();
  let child_events = events.clone();
  let child_store = SessionStore::default_store()?;
  let child_profile = mcp_profile.clone();
  let child_mesh = config.mesh_active() && synapse.is_some();
  let subagents: SubagentHandler = Arc::new(move |request: SubagentRequest| {
    let SubagentRequest {
      parent_id,
      name,
      prompt,
      role,
    } = request;
    let provider = child_provider.clone();
    let mut tools = child_tools.clone();
    let workspace = child_workspace.clone();
    let mut options = child_options.clone();
    let approver = child_approver.clone();
    let events = child_events.clone();
    let store = child_store.clone();
    let profile = child_profile.clone();
    Box::pin(async move {
      let mut session = Session::child(workspace.clone(), parent_id);
      // a guardian on the mesh gets its own client, so it holds a seat of its own rather
      // than renaming the one its parent registered
      if child_mesh {
        let hub = Arc::new(McpHub::new(profile));
        let synapse = Synapse::new(hub.clone(), workspace.clone());
        let role = role.unwrap_or_else(|| "worker".into());
        match synapse.register(&name, &role).await {
          Ok(roster) => {
            tools.replace(hub.tool());
            options.instructions.push_str(&format!(
              "\n\nYou are on the Synapse agent mesh as `{name}`. Other agents can message \
               you; use the mcp tool's synapse/inbox to read what arrived, synapse/send to \
               answer, and synapse/reportstatus to say what you are doing.\n{}",
              roster.trim()
            ));
          }
          Err(error) => events.emit(Event::Error {
            message: format!("{name} could not join the mesh: {error:#}"),
          }),
        }
      }
      events.emit(Event::SubagentStart {
        session_id: session.id.to_string(),
        parent_id: parent_id.to_string(),
        name: name.clone(),
      });
      let session_id = session.id.to_string();
      let forwarded = events.clone();
      let child_events = EventSink::new(move |event| {
        forwarded.emit(Event::SubagentEvent {
          session_id: session_id.clone(),
          event: Box::new(event),
        });
      });
      let agent = Agent::new(provider, tools, workspace, child_events, approver);
      let result = agent.run(&mut session, prompt, options).await;
      store.save(&session).await?;
      events.emit(Event::SubagentEnd {
        session_id: session.id.to_string(),
        error: result.is_err(),
      });
      Ok(SubagentResult {
        session_id: session.id,
        name,
        output: result?,
        usage: session.usage,
      })
    })
  });
  tools.insert(subagent_tool(SubagentRegistry::new(subagents), child_mesh))?;
  Ok((
    Agent::new(provider, tools, workspace.to_path_buf(), events, approver),
    options,
  ))
}

pub async fn run_once(
  workspace: PathBuf,
  config: Config,
  mut session: Session,
  prompt: String,
  image_paths: Vec<PathBuf>,
  json: bool,
  save: bool,
) -> Result<()> {
  let (agent, options) = make_agent(&workspace, &config, json).await?;
  output_events(json).emit(Event::SessionStart {
    session_id: session.id.to_string(),
  });
  let images = load_images(&image_paths).await?;
  let result = if images.is_empty() {
    agent.run(&mut session, prompt, options).await.map(|_| ())
  } else {
    agent
      .run_with_images(&mut session, prompt, images, options)
      .await
      .map(|_| ())
  };
  if save {
    SessionStore::default_store()?.save(&session).await?;
  }
  result?;
  if !json {
    println!();
  }
  Ok(())
}

pub async fn interactive(
  workspace: PathBuf,
  mut config: Config,
  mut session: Session,
  mut initial_prompt: Option<String>,
) -> Result<()> {
  let store = SessionStore::default_store()?;
  let prompts = PromptCatalog::discover(&workspace).await?;
  let mut lines = BufReader::new(tokio::io::stdin()).lines();
  if config.model.trim().is_empty() || config.active_provider().is_err() {
    configure(&mut config, &mut lines).await?;
    offer_synapse(&mut config).await?;
  }
  config.validate()?;
  if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
    loop {
      let outcome = crate::tui::run_chat(
        workspace.clone(),
        &mut config,
        session,
        initial_prompt.take(),
      )
      .await?;
      session = outcome.session;
      store.save(&session).await?;
      match outcome.next {
        crate::tui::ChatNext::Quit => return Ok(()),
        crate::tui::ChatNext::Configure => {
          crate::tui::configure(&mut config).await?;
          config.validate()?;
        }
        crate::tui::ChatNext::Import => {
          crate::tui::import(&workspace, &config).await?;
        }
        crate::tui::ChatNext::Settings => {
          let headers = ainz::HeaderCatalog::discover(&workspace).await?;
          if crate::tui::settings(&mut config, &headers).await? {
            crate::tui::configure(&mut config).await?;
          }
          config.validate()?;
        }
      }
    }
  }
  let (mut agent, mut options) = make_agent(&workspace, &config, false).await?;
  print_header(&config);
  println!("/config changes provider and model · /image PATH PROMPT attaches an image");
  println!(
    "session {} · /exit quits · /history lists nodes · /checkout NODE branches",
    session.id
  );
  if config.permissions != PermissionMode::Ask {
    println!("type during a run to steer · /cancel interrupts the active run");
  }
  loop {
    let line = match initial_prompt.take() {
      Some(prompt) => prompt,
      None => {
        print!("\n> ");
        std::io::stdout().flush()?;
        let Some(line) = lines.next_line().await? else {
          break;
        };
        line
      }
    };
    let prompt = line.trim();
    if prompt.is_empty() {
      continue;
    }
    if matches!(prompt, "/exit" | "/quit") {
      break;
    }
    if prompt == "/new" {
      session = Session::new(workspace.clone());
      println!("session {}", session.id);
      continue;
    }
    if prompt == "/history" {
      print_history(&session);
      continue;
    }
    if prompt == "/config" {
      configure(&mut config, &mut lines).await?;
      config.validate()?;
      (agent, options) = make_agent(&workspace, &config, false).await?;
      print_header(&config);
      continue;
    }
    if let Some(value) = prompt.strip_prefix("/checkout ") {
      let cursor = value
        .trim()
        .parse()
        .context("checkout node must be a UUID")?;
      session.checkout(Some(cursor))?;
      store.save(&session).await?;
      println!("cursor {cursor}");
      continue;
    }
    let mut images = Vec::new();
    let prompt = if let Some(value) = prompt.strip_prefix("/image ") {
      let (path, prompt) = value
        .split_once(char::is_whitespace)
        .context("usage: /image PATH PROMPT")?;
      images.push(Image::from_path(&PathBuf::from(path)).await?);
      prompt.trim()
    } else {
      prompt
    };
    let expanded = expand_prompt(prompt, &prompts).await?;
    let result = if config.permissions == PermissionMode::Ask {
      if images.is_empty() {
        agent.run(&mut session, expanded, options.clone()).await
      } else {
        agent
          .run_with_images(&mut session, expanded, images, options.clone())
          .await
      }
    } else {
      controlled_interactive(&agent, &mut session, expanded, images, &options, &mut lines).await
    };
    if let Err(error) = result {
      eprintln!("\nerror: {error:#}");
    }
    store.save(&session).await?;
  }
  Ok(())
}

// the workspace name, with a short suffix so two sessions in one checkout can both register
fn mesh_name(workspace: &std::path::Path) -> String {
  let base = workspace
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_else(|| "ainz".into());
  let suffix: String = uuid::Uuid::now_v7()
    .simple()
    .to_string()
    .chars()
    .take(4)
    .collect();
  format!("{base}-{suffix}")
}

// after first-run setup only, and only when an install is actually there
async fn offer_synapse(config: &mut Config) -> Result<()> {
  if config.synapse_active() || synapse::binary(&config.synapse).is_none() {
    return Ok(());
  }
  if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
    crate::tui::offer_synapse(config).await?;
    return Ok(());
  }
  println!(
    "Synapse is installed; `ainz synapse enable` shares memory with your other tools — {}",
    synapse::SITE
  );
  Ok(())
}

fn print_header(config: &Config) {
  println!(
    "Ainz · {} · {}{}",
    config.provider.as_deref().unwrap_or("default"),
    config.model,
    if config.yeet {
      " · yeet: no approval prompts, and unapproved plugins load"
    } else {
      ""
    }
  );
}

async fn configure(
  config: &mut Config,
  lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) -> Result<()> {
  if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
    return crate::tui::configure(config).await;
  }
  println!("\nAinz setup");
  println!("  1  Ollama");
  println!("  2  LiteLLM proxy");
  println!("  3  Codex CLI (headless)");
  println!("  4  Claude Code (headless)");
  println!("  5  Custom HTTP endpoint");
  println!("  6  Custom process");
  if !config.providers.is_empty() {
    println!("  7  Existing provider");
  }
  let choice = read_value(lines, "provider", None).await?;
  let (name, mut profile) = match choice.as_str() {
    "1" | "ollama" => ("ollama".to_string(), preset_profile(ProviderPreset::Ollama)),
    "2" | "litellm" => {
      let name = read_value(lines, "name", Some("litellm")).await?;
      let endpoint = read_value(lines, "endpoint", Some("http://127.0.0.1:4000/v1")).await?;
      let api_key_env = read_value(
        lines,
        "API key environment variable",
        Some("LITELLM_API_KEY"),
      )
      .await?;
      (name, ProviderConfig::http(endpoint, api_key_env))
    }
    "3" | "codex" => ("codex".to_string(), preset_profile(ProviderPreset::Codex)),
    "4" | "claude" | "claude-code" => {
      let mut profile = preset_profile(ProviderPreset::ClaudeCode);
      profile.models = vec!["fable".into(), "opus".into(), "sonnet".into()];
      ("claude".to_string(), profile)
    }
    "5" | "http" => {
      let name = read_value(lines, "name", Some("http")).await?;
      let endpoint = read_value(lines, "endpoint", Some("http://127.0.0.1:11434/v1")).await?;
      let api_key_env = read_value(lines, "API key environment variable", Some("")).await?;
      (name, ProviderConfig::http(endpoint, api_key_env))
    }
    "6" | "process" => {
      let name = read_value(lines, "name", Some("process")).await?;
      let command = read_value(lines, "command", None).await?;
      let args = read_value(lines, "arguments", Some("")).await?;
      let json = read_value(lines, "JSON result field? [y/N]", Some("n")).await?;
      (
        name,
        ProviderConfig::process(
          command,
          args.split_whitespace().map(str::to_string).collect(),
          if matches!(json.as_str(), "y" | "yes") {
            ProcessOutput::JsonResult
          } else {
            ProcessOutput::Text
          },
        ),
      )
    }
    "7" if !config.providers.is_empty() => choose_existing(config, lines).await?,
    _ => anyhow::bail!("unknown provider selection {choice}"),
  };

  if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
    anyhow::bail!("provider name must be non-empty and contain no whitespace");
  }
  if profile.kind == ProviderKind::Http && profile.models.is_empty() {
    let key = (!profile.api_key_env.is_empty())
      .then(|| std::env::var(&profile.api_key_env).ok())
      .flatten()
      .filter(|key| !key.is_empty());
    let provider = HttpProvider::new(
      profile
        .endpoint
        .clone()
        .context("HTTP provider requires an endpoint")?,
      String::new(),
      key,
    )?;
    match provider.models().await {
      Ok(models) => profile.models = models,
      Err(error) => eprintln!("could not ask the endpoint which models it serves: {error:#}"),
    }
  }

  let model = choose_model(config, &name, &profile, lines).await?;
  if !profile.models.contains(&model) {
    profile.models.push(model.clone());
    profile.models.sort();
  }
  profile.validate(&name)?;
  config.providers.insert(name.clone(), profile);
  config.provider = Some(name.clone());
  config.model = model.clone();
  config.save().await?;
  println!("configured {name} · {model}");
  Ok(())
}

async fn choose_existing(
  config: &Config,
  lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) -> Result<(String, ProviderConfig)> {
  let providers: Vec<_> = config.providers.iter().collect();
  for (index, (name, provider)) in providers.iter().enumerate() {
    println!("  {}  {} ({:?})", index + 1, name, provider.kind);
  }
  let value = read_value(lines, "existing provider", None).await?;
  let index = value
    .parse::<usize>()
    .ok()
    .filter(|index| *index > 0 && *index <= providers.len())
    .context("provider selection must be a listed number")?;
  let (name, provider) = providers[index - 1];
  Ok((name.clone(), provider.clone()))
}

async fn choose_model(
  config: &Config,
  name: &str,
  provider: &ProviderConfig,
  lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) -> Result<String> {
  if provider.models.is_empty() {
    return read_value(lines, "model", None).await;
  }
  println!("models");
  for (index, model) in provider.models.iter().enumerate() {
    println!("  {}  {}", index + 1, model);
  }
  let default =
    if config.provider.as_deref() == Some(name) && provider.models.contains(&config.model) {
      config.model.as_str()
    } else {
      provider.models[0].as_str()
    };
  let value = read_value(lines, "model", Some(default)).await?;
  Ok(
    value
      .parse::<usize>()
      .ok()
      .filter(|index| *index > 0 && *index <= provider.models.len())
      .map_or(value, |index| provider.models[index - 1].clone()),
  )
}

async fn read_value(
  lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
  label: &str,
  default: Option<&str>,
) -> Result<String> {
  match default {
    Some(default) if !default.is_empty() => print!("{label} [{default}]: "),
    _ => print!("{label}: "),
  }
  std::io::stdout().flush()?;
  let value = lines.next_line().await?.context("setup cancelled")?;
  let value = value.trim();
  if value.is_empty() {
    return default
      .map(str::to_string)
      .context(format!("{label} is required"));
  }
  Ok(value.to_string())
}

async fn controlled_interactive(
  agent: &Agent<RuntimeProvider>,
  session: &mut Session,
  prompt: String,
  images: Vec<Image>,
  options: &RunOptions,
  lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) -> Result<String> {
  let (controller, mut inbox) = run_control();
  let execution = async {
    if images.is_empty() {
      agent
        .run_controlled(session, prompt, options.clone(), &mut inbox)
        .await
    } else {
      agent
        .run_controlled_with_images(session, prompt, images, options.clone(), &mut inbox)
        .await
    }
  };
  tokio::pin!(execution);
  let mut input_open = true;
  loop {
    tokio::select! {
      result = &mut execution => break result,
      line = lines.next_line(), if input_open => {
        match line? {
          Some(line) if line.trim() == "/cancel" => {
            controller.cancel();
            eprintln!("\n↳ cancellation requested");
          }
          Some(line) if !line.trim().is_empty() => {
            controller.steer(line);
            eprintln!("\n↳ steering queued");
          }
          Some(_) => {}
          None => {
            input_open = false;
            controller.cancel();
          }
        }
      }
    }
  }
}

fn print_history(session: &Session) {
  for node in &session.nodes {
    println!(
      "{}  {:?}  {}{}",
      node.id,
      node.message.role,
      node
        .message
        .content
        .as_deref()
        .unwrap_or("[tool call]")
        .chars()
        .take(72)
        .collect::<String>(),
      if session.cursor == Some(node.id) {
        "  ← cursor"
      } else {
        ""
      }
    );
  }
}

pub(crate) async fn expand_prompt(prompt: &str, prompts: &PromptCatalog) -> Result<String> {
  if let Some(command) = prompt.strip_prefix('/') {
    let parts: Vec<_> = command.split_whitespace().map(str::to_string).collect();
    if let Some((name, args)) = parts.split_first()
      && prompts.prompts.iter().any(|prompt| prompt.name == *name)
    {
      return prompts.expand(name, args).await;
    }
  }
  Ok(prompt.to_string())
}

async fn load_images(paths: &[PathBuf]) -> Result<Vec<Image>> {
  let mut images = Vec::with_capacity(paths.len());
  for path in paths {
    images.push(Image::from_path(path).await?);
  }
  Ok(images)
}

fn output_events(json: bool) -> EventSink {
  EventSink::new(move |event| {
    if json {
      println!("{}", serde_json::to_string(&event).unwrap_or_default());
      return;
    }
    match event {
      Event::TextDelta { text } => {
        print!("{text}");
        drop(std::io::stdout().flush());
      }
      Event::ToolStart { call } => eprintln!("\n→ {}", call.name),
      Event::ToolEnd {
        output,
        error: true,
        ..
      } => eprintln!("  {output}"),
      _ => {}
    }
  })
}

// the prompt reads the terminal directly on a blocking thread so the runtime keeps streaming
fn approve(call: &ToolCall, risk: Risk) -> Approval {
  let name = call.name.clone();
  let arguments = call.arguments.to_string();
  Box::pin(async move {
    tokio::task::spawn_blocking(move || {
      eprint!("\nallow {name} ({risk:?}) {arguments}? [y/N] ");
      drop(std::io::stderr().flush());
      let mut answer = String::new();
      std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "yes")
    })
    .await
    .unwrap_or(false)
  })
}
