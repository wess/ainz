# AgentX

AgentX is a small agent harness built in Rust on Tokio. It keeps the model loop, tools,
history, and extensions available as library primitives instead of burying them in the
terminal interface.

The current foundation includes:

- streamed and non-streamed chat-completions transport
- interactive, one-shot, and newline-delimited JSON output modes
- resumable tree-structured sessions
- automatic branch-aware context compaction with persisted summaries
- persisted token usage totals
- workspace-scoped read, list, search, write, edit, and shell tools
- durable background jobs with persisted status, output, and safe process-group stopping
- library-level steering and cancellation channels with safe turn-boundary delivery
- delegated subagent runs with inherited policy and durable parent-linked sessions
- persisted multimodal messages with local PNG, JPEG, GIF, and WebP attachments
- ask, automatic, and read-only permission modes
- hierarchical `AGENTS.md` instructions
- lazy `SKILL.md` discovery through one context-efficient tool
- scoped Markdown prompt templates with positional arguments
- lazy external tool discovery over stdio and Streamable HTTP
- scoped component and process plugins with static tool schemas and capabilities
- content-pinned plugin approval covering manifests and runtime artifacts

## Install

With Homebrew on macOS or Linux:

```sh
brew install wess/packages/agentx
```

With the checksum-verifying installer on macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/wess/agentx/v0.1.0/install.sh | sh
```

Release downloads support Intel and Apple Silicon macOS and x86_64 and arm64 Linux. See
[`docs/install.md`](docs/install.md) for pinned versions, custom install directories, Cargo, and
uninstall instructions. The project site includes a [tutorial](https://wess.github.io/agentx/tutorial/)
and [reference manual](https://wess.github.io/agentx/docs/).

## Build from source

```sh
cargo build --release
# or install the CLI from this checkout
cargo install --path .
```

The executable, crate, environment variables, config roots, project folders, and plugin WIT
namespace all use `agentx`. On first launch, AgentX carries forward an existing configuration and
MCP profile from the former `struts` user root. It continues to discover older sessions, headers,
prompts, skills, plugins, and approvals while new state is written under `agentx` and `.agentx`.

Set a model and, when needed, a key:

```sh
export AGENTX_MODEL=your-model
export AGENTX_API_KEY=your-key
```

The default endpoint is `http://127.0.0.1:11434/v1`. Override it with
`AGENTX_ENDPOINT` or `~/.config/agentx/config.toml`:

```toml
endpoint = "https://gateway.example/v1"
model = "your-model"
api_key_env = "AGENTX_API_KEY"
max_steps = 32
max_output_bytes = 65536
context_tokens = 128000
compact_at_tokens = 96000
preserve_messages = 8
permissions = "ask"
```

## Use

```sh
# interactive
agentx

# one request
agentx ask "inspect the project and run its tests"

# multimodal request
agentx ask --image screenshot.png "explain this interface"

# machine-readable event stream
agentx ask --json "summarize this workspace"

# persistent JSON-RPC process for editor and application integration
agentx rpc

# sessions and extensions
agentx sessions
agentx resume
agentx resume SESSION_ID --at NODE_ID "continue from this branch"
agentx skills
agentx prompts
agentx usage
agentx mcp
agentx mcp add synapse --required -- /absolute/path/to/synapse mcp
agentx providers list
agentx models list
agentx plugins list
agentx doctor
```

JSON mode never opens an approval prompt. Use `--permissions auto` explicitly when a
noninteractive request is allowed to write files or execute commands.
Interactive terminals use a terminal-native Ratatui interface with a streaming transcript,
permission prompts, tool activity, and a subagent roster. `Ctrl+L` toggles the roster and
remembers its visibility across runs,
`Ctrl+1` selects the primary transcript, `Ctrl+2` through `Ctrl+9` select subagents, and
`Ctrl++` / `Ctrl+-` cycle through every agent. Subagent panes are view-only. `Ctrl+C` cancels
an active run, and text entered during a primary run is queued as steering for the next safe
turn boundary. The status bar shows the active transcript's cumulative token usage, permission
mode, running agents, and current tool or run state. Redirected and scripted input keeps the
plain line interface.

The empty transcript opens with one of ten pixel-rendered `AGENTX` mastheads chosen per launch. The
collection spans graffiti, chrome, inferno, toxic, ice, orbital, industrial, abyssal, and character-art
compositions. Each uses layered color cells, highlights, outlines, extrusion, and shadow rather than
flat character art. The masthead contracts when the agent roster narrows the transcript and stays
stable for the whole run.

Custom UTF-8 ASCII or ANSI-SGR mastheads can be placed in `~/.config/agentx/headers/` or a project's
`.agentx/headers/` directory. `/headers` lists artwork and validation errors; `/header NAME` selects
and remembers one, while `/header random` mixes custom art into the built-in rotation. See
[`docs/headers.md`](docs/headers.md) for the safe color format, size limits, and creator guidelines.

Type `/` to open the command palette. Continue typing to fuzzy-search command names, usage, and
descriptions; use Up/Down to select, Tab to complete, Enter to accept or run, and Escape to close.
The palette includes built-in session, provider, permission, agent, status, and extension commands
alongside prompt templates discovered from the user and workspace configuration.

## Providers

Provider and model profiles can be managed without editing `config.toml`:

Running `agentx` with no configured model opens the setup flow automatically. Use `/config`
inside an interactive session to add or switch providers and models without restarting.

```sh
# local HTTP server with live model discovery
agentx providers add ollama --preset ollama
agentx models list ollama --refresh
agentx providers use ollama qwen3:8b

# authenticated coding CLIs in headless mode
agentx providers add codex --preset codex --known-model gpt-5.6-sol
agentx providers use codex gpt-5.6-sol

agentx providers add claude --preset claude-code --known-model sonnet
agentx providers use claude sonnet
```

HTTP profiles use AgentX's own model and tool loop. Headless process profiles run their own
agent loop and return the final response as one AgentX assistant turn. In `ask` and
`read_only` modes they run read-only; `auto` permits workspace edits. See
[`docs/providers.md`](docs/providers.md) for custom endpoints, commands, and limitations.

## Plugins

Native plugins are discovered from `~/.config/agentx/plugins/*/plugin.toml` and from
`.agentx/plugins/*/plugin.toml` between the filesystem root and the workspace. Portable Agent
Plugins 1.0 packages using `plugin.json`, `skills/`, and `mcp.json` are also discovered from those
locations and the shared `~/.agents/plugins` or `.agents/plugins` roots. The
nearest project definition wins when names collide.

Discovery does not execute a plugin. Approve the exact plugin content before its tools become
available:

```sh
agentx plugins approve example
```

Changing `plugin.toml` or its runtime artifact returns it to pending. See
[`docs/plugins.md`](docs/plugins.md) for the manifest and protocol.

Component plugins run with fuel, memory, instance, table, and wall-clock limits and no
inherited host authority. Capability-checked imports selectively expose workspace reads,
writes, processes, or HTTP access per tool. Process plugins remain trusted native
programs; capabilities drive visibility and per-call approval but cannot sandbox one.

## External tools

Stdio and Streamable HTTP servers can be registered from the CLI and are persisted only in
the platform user profile; cloning a repository cannot start one. Harnesses such as Synapse
may provide an explicit, ephemeral launch profile with `--mcp-config`. Schemas remain out of
the model context until it searches for a tool and requests the matching schema. See
[`docs/mcp.md`](docs/mcp.md).
