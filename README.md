# AgentX

AgentX is a small agent harness built in Rust on Tokio. The model loop, tools, session
history, and extensions are library primitives; the terminal interface is one consumer of
them, alongside one-shot, JSON event stream, and JSON-RPC modes.

It talks to OpenAI-compatible chat-completions endpoints with streaming and tool calls, and
to headless coding CLIs as process providers. Sessions are resumable trees with automatic,
branch-aware compaction. Tools cover workspace read, list, search, write, edit, and shell,
durable background jobs, lazily loaded skills, prompt templates, subagents, MCP servers over
stdio and Streamable HTTP, and content-pinned WebAssembly component or process plugins.

## Install

With Homebrew on macOS or Linux:

```sh
brew install wess/packages/agentx
```

With the checksum-verifying installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/wess/agentx/main/install.sh | sh
```

Release downloads cover Intel and Apple Silicon macOS and x86_64 and arm64 Linux. The
`search` tool shells out to [ripgrep](https://github.com/BurntSushi/ripgrep), so install `rg`
as well. See [`docs/install.md`](docs/install.md) for pinned versions, custom install
directories, Cargo, and uninstalling. The project site has a
[tutorial](https://wess.io/agentx/tutorial/) and [reference manual](https://wess.io/agentx/docs/).

## Build from source

```sh
cargo build --release
# or install the CLI from this checkout
cargo install --path .
```

## Configure

Running `agentx` with no configured model opens the setup flow. `/config` inside a session
adds or switches providers without restarting, and the same operations are scriptable:

```sh
agentx providers add ollama --preset ollama
agentx models list ollama --refresh
agentx providers use ollama qwen3:8b

agentx providers add codex --preset codex --known-model gpt-5.6-sol
agentx providers add claude --preset claude-code --known-model sonnet
```

The config file lives in the platform config directory: `~/Library/Application Support/agentx/config.toml`
on macOS, `~/.config/agentx/config.toml` on Linux. `AGENTX_CONFIG` overrides the path, and
`AGENTX_MODEL`, `AGENTX_ENDPOINT`, `AGENTX_PROVIDER`, and `AGENTX_API_KEY` override values.

```toml
provider = "ollama"
model = "qwen3:8b"
max_steps = 32
max_output_bytes = 65536
context_tokens = 128000
compact_at_tokens = 96000
preserve_messages = 8
permissions = "ask"

[providers.ollama]
kind = "http"
endpoint = "http://127.0.0.1:11434/v1"
```

Without a `provider`, the legacy top-level `endpoint` and `api_key_env` keys describe one HTTP
provider. HTTP profiles use AgentX's own model and tool loop. Process profiles run a coding CLI's
own loop and return its final response as one assistant turn; in `ask` and `read_only` modes
they run read-only, and `auto` permits workspace edits. See
[`docs/providers.md`](docs/providers.md).

## Use

```sh
agentx                                    # interactive
agentx ask "inspect the project and run its tests"
agentx ask --image screenshot.png "explain this interface"
agentx ask --json "summarize this workspace"   # machine-readable event stream
agentx rpc                                # persistent JSON-RPC process

agentx sessions
agentx resume
agentx resume SESSION_ID --at NODE_ID "continue from this branch"
agentx skills
agentx prompts
agentx usage
agentx mcp
agentx plugins list
agentx doctor
```

JSON mode never opens an approval prompt; pass `--permissions auto` explicitly when a
noninteractive request may write files or execute commands.

In a terminal, AgentX runs a Ratatui interface with a streaming transcript, permission prompts
that show the tool's arguments, tool activity, and a subagent roster. Type `/` to open the
command palette and fuzzy-search commands and prompt templates. `Ctrl+L` toggles the roster
and remembers the choice, `Ctrl+1` selects the primary transcript, `Ctrl+2` through `Ctrl+9`
select subagents, and `Ctrl+=` / `Ctrl+-` cycle through them; those chords need a terminal
that speaks the kitty keyboard protocol, and `/agent N` works everywhere. `Ctrl+C` cancels an
active run, and a second `Ctrl+C` abandons a run whose provider ignores the cancel. Text
entered during a run is queued as steering for the next safe turn boundary. Redirected input
keeps a plain line interface.

An empty transcript shows one of ten built-in mastheads. Paint your own in the
[masthead studio](https://wess.io/agentx/masthead/), or bring UTF-8 ASCII or ANSI-SGR art, and
drop it in the config directory's `headers/` folder or a project's `.agentx/headers/`.
`/headers` lists them and `/header NAME` selects one; see [`docs/headers.md`](docs/headers.md).

## Instructions, skills, and prompts

`AGENTS.md` files from the filesystem root down to the workspace are appended to the system
instructions, nearest last. `SKILL.md` directories under `skills/`, `.agents/skills/`,
`.agentx/skills/`, and the config directory are discovered up front and loaded through one
`skill` tool only when the model asks for them. Markdown prompt templates in `prompts/`
directories expand `{{args}}` and `{{1}}`-style positional arguments as `/name ARGS`.

## External tools

Stdio servers are registered from the CLI; Streamable HTTP servers are added to the profile
file directly. The profile lives in the user config directory only, so cloning a repository
cannot start a server. A harness such as Synapse may pass an ephemeral launch profile with
`--mcp-config`. Schemas stay out of the model context until it searches for a tool and asks for
one schema. See [`docs/mcp.md`](docs/mcp.md).

```sh
agentx mcp add synapse --required -- /absolute/path/to/synapse mcp
```

## Plugins

Native plugins are discovered from the config directory's `plugins/` folder and from
`.agentx/plugins/*/plugin.toml` between the filesystem root and the workspace. Portable Agent
Plugins 1.0 packages using `plugin.json`, `skills/`, and `mcp.json` are also discovered there
and from `~/.agents/plugins` or `.agents/plugins`. The nearest project definition wins when
names collide.

Discovery reads manifests but never runs anything. Approve the exact content before a plugin's
tools become available:

```sh
agentx plugins list
agentx plugins approve example
agentx plugins revoke example
```

The approval pins the manifest, every file under the plugin directory, and the executable or
component it names; changing any of them returns the plugin to pending, and the artifact is
rechecked when it runs. Component plugins run with fuel, memory, instance, table, and
wall-clock limits, no WASI authority, and capability-checked host imports per tool. Process
plugins are trusted native programs; capabilities drive visibility and approval but cannot
sandbox one. See [`docs/plugins.md`](docs/plugins.md).
