use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use uuid::Uuid;

use ainz::{Config, PermissionMode, Session};

mod app;
mod command;
mod live;
mod rpc_cli;
mod tui;

use command::{
  JobCommand, McpCommand, MemoryCommand, ModelCommand, PluginCommand, ProviderCommand,
  SessionsCommand, SkillCommand, SynapseCommand,
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
  /// Start with this request, or use -p to print one answer and exit.
  prompt: Option<String>,
  #[arg(short = 'p', long)]
  print: bool,
  #[arg(long, default_value = "text", value_parser = ["text", "json"])]
  output_format: String,
  /// Continue the latest saved conversation in this workspace.
  #[arg(short = 'c', long)]
  continue_session: bool,
  /// Keep this interactive session running after detaching
  #[arg(long, conflicts_with = "no_detach")]
  detach: bool,
  /// Run in the foreground even when background sessions are enabled
  #[arg(long)]
  no_detach: bool,
  #[arg(long, hide = true)]
  live_worker: Option<Uuid>,
  #[arg(long, global = true)]
  workspace: Option<PathBuf>,
  #[arg(long, global = true)]
  model: Option<String>,
  #[arg(long, global = true)]
  endpoint: Option<String>,
  #[arg(long, global = true)]
  provider: Option<String>,
  #[arg(long = "fallback-provider", global = true)]
  fallback_providers: Vec<String>,
  /// Stop after the provider-reported cost reaches this amount in USD.
  #[arg(long, global = true)]
  budget_usd: Option<f64>,
  #[arg(long, global = true)]
  mcp_config: Option<PathBuf>,
  #[arg(long, global = true)]
  initial_prompt: Option<String>,
  #[arg(long, global = true, value_enum)]
  permissions: Option<PermissionArg>,
  /// Run wide open: every tool call is allowed without asking
  #[arg(long, global = true, alias = "yolo")]
  yeet: bool,
  #[command(subcommand)]
  command: Option<Command>,
}

#[derive(Clone, Copy, ValueEnum)]
enum PermissionArg {
  Ask,
  Auto,
  ReadOnly,
}

impl From<PermissionArg> for PermissionMode {
  fn from(value: PermissionArg) -> Self {
    match value {
      PermissionArg::Ask => Self::Ask,
      PermissionArg::Auto => Self::Auto,
      PermissionArg::ReadOnly => Self::ReadOnly,
    }
  }
}

#[derive(Subcommand)]
enum Command {
  /// Propose and check a harness improvement using recent runs
  Improve(command::improve::ImproveArgs),
  #[command(hide = true)]
  Host { id: Uuid },
  /// Reconnect to a live session by id or unique name; omit to choose
  Attach { id: Option<String> },
  /// Stop a background session (its saved conversation remains)
  Stop { id: String },
  Ask {
    prompt: String,
    #[arg(long = "image")]
    images: Vec<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    no_save: bool,
  },
  Resume {
    id: Option<Uuid>,
    prompt: Option<String>,
    #[arg(long)]
    at: Option<Uuid>,
    #[arg(long = "image")]
    images: Vec<PathBuf>,
    #[arg(long)]
    json: bool,
  },
  Sessions {
    #[arg(long, conflicts_with = "search")]
    live: bool,
    #[command(subcommand)]
    command: Option<SessionsCommand>,
    #[arg(long)]
    search: Option<String>,
    #[arg(long)]
    json: bool,
  },
  Skills {
    #[command(subcommand)]
    command: Option<SkillCommand>,
    #[arg(long)]
    json: bool,
  },
  Memory {
    #[command(subcommand)]
    command: MemoryCommand,
  },
  Import {
    names: Vec<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    json: bool,
  },
  Synapse {
    #[command(subcommand)]
    command: Option<SynapseCommand>,
  },
  Prompts {
    name: Option<String>,
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
    #[arg(long)]
    json: bool,
  },
  Usage {
    #[arg(long)]
    json: bool,
  },
  Mcp {
    #[command(subcommand)]
    command: Option<McpCommand>,
    #[arg(long)]
    json: bool,
  },
  Rpc {
    #[arg(long)]
    no_save: bool,
  },
  Plugins {
    #[command(subcommand)]
    command: PluginCommand,
  },
  Providers {
    #[command(subcommand)]
    command: ProviderCommand,
  },
  Models {
    #[command(subcommand)]
    command: ModelCommand,
  },
  Jobs {
    #[command(subcommand)]
    command: Option<JobCommand>,
  },
  /// List reusable agent definitions.
  Agents,
  Checkpoint {
    id: Option<Uuid>,
    #[arg(default_value = "manual")]
    label: String,
  },
  Restore {
    id: Uuid,
    #[arg(default_value = "manual")]
    label: String,
  },
  /// Configure a provider and model.
  Login,
  /// List discovered masthead artwork.
  Headers,
  /// Write workspace instructions with the active model.
  Init,
  Doctor {
    /// Send a small billable inference request, without retries or fallback.
    #[arg(long)]
    check_provider: bool,
  },
}

#[tokio::main(worker_threads = 2)]
async fn main() {
  if let Err(error) = run().await {
    eprintln!("error: {error:#}");
    std::process::exit(1);
  }
}

async fn run() -> Result<()> {
  let mut cli = Cli::parse();
  let workspace = cli
    .workspace
    .unwrap_or(std::env::current_dir()?)
    .canonicalize()?;
  let mut config = Config::load().await?;
  if let Some(model) = cli.model {
    config.model = model;
  }
  if let Some(endpoint) = cli.endpoint {
    config.endpoint = endpoint;
    config.provider = None;
  }
  if let Some(provider) = cli.provider {
    config.provider = Some(provider);
  }
  if !cli.fallback_providers.is_empty() {
    config.fallback_providers = cli.fallback_providers;
  }
  if let Some(budget) = cli.budget_usd {
    config.budget_usd = Some(budget);
  }
  config.mcp_config = cli.mcp_config;
  if let Some(permissions) = cli.permissions {
    config.permissions = permissions.into();
  }
  if cli.yeet {
    config.permissions = PermissionMode::Auto;
    config.yeet = true;
  }

  if cli.print && cli.command.is_some() {
    anyhow::bail!("--print cannot be combined with a subcommand");
  }
  if cli.continue_session && cli.command.is_some() {
    anyhow::bail!("--continue-session cannot be combined with a subcommand");
  }
  if cli.print {
    use std::io::IsTerminal;
    use tokio::io::AsyncReadExt;
    let mut prompt = cli
      .prompt
      .take()
      .or_else(|| cli.initial_prompt.take())
      .unwrap_or_default();
    if !std::io::stdin().is_terminal() {
      let mut input = String::new();
      tokio::io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut input)
        .await?;
      if input.len() > 16 * 1024 * 1024 {
        anyhow::bail!("piped input exceeds 16 MiB");
      }
      if !input.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&input);
      }
    }
    if prompt.trim().is_empty() {
      anyhow::bail!("--print needs a prompt or piped input");
    }
    config.validate()?;
    let session = if cli.continue_session {
      command::load_session(&workspace, None).await?
    } else {
      Session::new(workspace.clone())
    };
    return app::run_once(
      workspace,
      config,
      session,
      prompt,
      Vec::new(),
      cli.output_format == "json",
      true,
    )
    .await;
  }
  if cli.continue_session {
    cli.command = Some(Command::Resume {
      id: None,
      prompt: None,
      at: None,
      images: Vec::new(),
      json: false,
    });
  }
  if let Some(prompt) = cli.prompt.take() {
    if cli.command.is_some() && !cli.continue_session {
      anyhow::bail!("use the subcommand’s prompt argument");
    }
    cli.initial_prompt = Some(prompt);
  }
  let interactive = matches!(
    &cli.command,
    None | Some(Command::Resume { prompt: None, .. })
  );
  if cli.detach && !interactive {
    anyhow::bail!("--detach applies to interactive sessions; use ainz attach to reconnect");
  }
  if cli.live_worker.is_none()
    && interactive
    && (cli.detach || (config.ui.background && !cli.no_detach))
  {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
      anyhow::bail!("background sessions need a terminal; use --no-detach for piped input");
    }
    let args = std::env::args_os()
      .skip(1)
      .filter(|arg| arg != "--detach" && arg != "--no-detach")
      .collect();
    let id = live::start(&workspace, args).await?;
    return live::attach(None, Some(id), &workspace).await;
  }
  let link = match cli.live_worker {
    Some(id) => Some(live::Link::open(id).await?),
    None => None,
  };
  match cli.command {
    Some(Command::Improve(args)) => command::improve::run(&workspace, &mut config, args).await,
    Some(Command::Host { id }) => live::serve(id).await,
    Some(Command::Attach { id }) => live::attach(id.as_deref(), None, &workspace).await,
    Some(Command::Stop { id }) => live::stop(&id).await,
    Some(Command::Sessions {
      live: true,
      json,
      command,
      ..
    }) => {
      if command.is_some() {
        anyhow::bail!("--live cannot be combined with export");
      }
      let infos = live::list().await?;
      if json {
        println!("{}", serde_json::to_string_pretty(&infos)?);
      } else {
        for info in infos {
          println!(
            "{}  {}  {}  {}{}",
            info.short(),
            info.badge(),
            info.title,
            info.workspace.display(),
            if info.attached { " (attached)" } else { "" }
          );
        }
      }
      Ok(())
    }
    Some(Command::Sessions {
      live: false,
      command,
      search,
      json,
    }) => match command {
      Some(SessionsCommand::Export { id, out }) => {
        command::export_session(&workspace, id, out.as_deref()).await
      }
      None => match search {
        Some(query) => command::search_sessions(&workspace, &query, json).await,
        None => command::list_sessions(&workspace, json).await,
      },
    },
    Some(Command::Skills { command, json }) => {
      command::skills(&workspace, &config, command, json).await
    }
    Some(Command::Memory { command }) => command::memory(&workspace, &mut config, command).await,
    Some(Command::Import {
      names,
      kind,
      all,
      json,
    }) => command::import(&workspace, &config, &names, kind.as_deref(), all, json).await,
    Some(Command::Synapse { command }) => command::synapse(&mut config, command).await,
    Some(Command::Prompts { name, args, json }) => {
      command::prompts(&workspace, name.as_deref(), &args, json).await
    }
    Some(Command::Usage { json }) => command::usage(&workspace, json).await,
    Some(Command::Mcp { command, json }) => command::mcp(command, json).await,
    Some(Command::Rpc { no_save }) => {
      config.validate()?;
      rpc_cli::run(workspace, config, no_save).await
    }
    Some(Command::Plugins { command }) => command::plugins(&workspace, command).await,
    Some(Command::Providers { command }) => command::providers(&mut config, command).await,
    Some(Command::Models { command }) => command::models(&mut config, command).await,
    Some(Command::Jobs { command }) => {
      command::jobs(&workspace, command, config.max_output_bytes).await
    }
    Some(Command::Agents) => {
      for agent in ainz::subagent::AgentCatalog::discover(&workspace)
        .await?
        .agents
      {
        println!("{}", agent.summary());
      }
      Ok(())
    }
    Some(Command::Checkpoint { id, label }) => {
      let session = command::load_session(&workspace, id).await?;
      let path = ainz::SessionStore::default_store()?
        .checkpoint(&session, &label)
        .await?;
      println!("checkpoint {label} saved at {}", path.display());
      Ok(())
    }
    Some(Command::Restore { id, label }) => {
      if live::list()
        .await?
        .iter()
        .any(|info| info.session == Some(id))
      {
        anyhow::bail!("stop the live session before restoring its checkpoint");
      }
      let store = ainz::SessionStore::default_store()?;
      let session = store.restore(id, &label).await?;
      if session.workspace != workspace {
        anyhow::bail!("checkpoint belongs to another workspace");
      }
      store.save(&session).await?;
      println!("restored session {id} from checkpoint {label}");
      Ok(())
    }
    Some(Command::Login) => app::configure_standalone(&mut config).await,
    Some(Command::Headers) => {
      let catalog = ainz::HeaderCatalog::discover(&workspace).await?;
      for header in catalog.headers {
        println!("{}", header.name);
      }
      Ok(())
    }
    Some(Command::Init) => {
      config.validate()?;
      let session = Session::new(workspace.clone());
      app::run_once(workspace, config, session, "Inspect this workspace and write a concise AGENTS.md with its build, test, architecture, and coding conventions. Preserve existing instructions and do not invent commands.".into(), Vec::new(), false, true).await
    }
    Some(Command::Doctor { check_provider }) => {
      command::doctor(&workspace, &config).await?;
      if check_provider {
        command::check_provider(&workspace, &config).await?;
      }
      Ok(())
    }
    Some(Command::Ask {
      prompt,
      images,
      json,
      no_save,
    }) => {
      config.validate()?;
      let session = Session::new(workspace.clone());
      app::run_once(workspace, config, session, prompt, images, json, !no_save).await
    }
    Some(Command::Resume {
      id,
      prompt,
      at,
      images,
      json,
    }) => {
      config.validate()?;
      let mut session = command::load_session(&workspace, id).await?;
      if let Some(cursor) = at {
        session.checkout(Some(cursor))?;
      }
      if let Some(prompt) = prompt {
        app::run_once(workspace, config, session, prompt, images, json, true).await
      } else {
        if !images.is_empty() {
          anyhow::bail!("--image requires a prompt when resuming");
        }
        app::interactive(workspace, config, session, cli.initial_prompt, link).await
      }
    }
    None => {
      app::interactive(
        workspace.clone(),
        config,
        Session::new(workspace),
        cli.initial_prompt,
        link,
      )
      .await
    }
  }
}
