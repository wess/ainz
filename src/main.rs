use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use uuid::Uuid;

use ainz::{Config, PermissionMode, Session};

mod app;
mod command;
mod rpc_cli;
mod tui;

use command::{
  McpCommand, MemoryCommand, ModelCommand, PluginCommand, ProviderCommand, SessionsCommand,
  SkillCommand, SynapseCommand,
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
  #[arg(long, global = true)]
  workspace: Option<PathBuf>,
  #[arg(long, global = true)]
  model: Option<String>,
  #[arg(long, global = true)]
  endpoint: Option<String>,
  #[arg(long, global = true)]
  provider: Option<String>,
  #[arg(long, global = true)]
  mcp_config: Option<PathBuf>,
  #[arg(long, global = true)]
  initial_prompt: Option<String>,
  #[arg(long, global = true, value_enum)]
  permissions: Option<PermissionArg>,
  /// Run wide open: every tool call is allowed without asking
  #[arg(long, global = true)]
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
  Doctor,
}

#[tokio::main]
async fn main() {
  if let Err(error) = run().await {
    eprintln!("error: {error:#}");
    std::process::exit(1);
  }
}

async fn run() -> Result<()> {
  let cli = Cli::parse();
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
  config.mcp_config = cli.mcp_config;
  if let Some(permissions) = cli.permissions {
    config.permissions = permissions.into();
  }
  if cli.yeet {
    config.permissions = PermissionMode::Auto;
    config.yeet = true;
  }

  match cli.command {
    Some(Command::Sessions {
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
    Some(Command::Doctor) => command::doctor(&workspace, &config).await,
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
        app::interactive(workspace, config, session, cli.initial_prompt).await
      }
    }
    None => {
      app::interactive(
        workspace.clone(),
        config,
        Session::new(workspace),
        cli.initial_prompt,
      )
      .await
    }
  }
}
