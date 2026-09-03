# Ainz

Ainz is a small agent harness built in Rust on Tokio. The model loop, tools, session
history, and extensions are library primitives; the terminal interface is one consumer of
them, alongside one-shot, JSON event stream, and JSON-RPC modes.

It talks to OpenAI-compatible chat-completions endpoints with streaming and tool calls,
including a LiteLLM proxy, and to headless coding CLIs as process providers. Sessions are
resumable trees with automatic, branch-aware compaction. Tools cover workspace read, list,
search, write, edit, and shell, reading a URL, a plan the session keeps as it works, durable
background jobs, lazily loaded skills, prompt templates, subagents, MCP servers over stdio and
Streamable HTTP, and content-pinned WebAssembly component or process plugins. Sessions remember across runs: durable memory kept locally or in Synapse,
search over earlier sessions, and skills a session writes for the next one.

## Install

With Homebrew on macOS or Linux:

```sh
brew install wess/packages/ainz
```

With the checksum-verifying installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/wess/ainz/main/install.sh | sh
```

Release downloads cover Intel and Apple Silicon macOS and x86_64 and arm64 Linux. The
`search` tool shells out to [ripgrep](https://github.com/BurntSushi/ripgrep), so install `rg`
as well. See [`docs/install.md`](docs/install.md) for pinned versions, custom install
directories, Cargo, and uninstalling. The project site has a
[tutorial](https://wess.io/ainz/tutorial/) and [reference manual](https://wess.io/ainz/docs/).

Ainz was previously called AgentX. On first launch it carries forward the configuration and MCP
profile from the old user directory, and it keeps discovering sessions, headers, prompts, skills,
plugins, and plugin approvals written under the old name while new state is written under `ainz`
and `.ainz`. The `AGENTX_*` environment variables are now `AINZ_*`.

## Build from source

```sh
cargo build --release
# or install the CLI from this checkout
cargo install --path .
```

## Configure

Running `ainz` with no configured model opens the setup flow. `/config` inside a session
adds or switches providers without restarting, and the same operations are scriptable:

```sh
ainz providers add ollama --preset ollama
ainz models list ollama --refresh
ainz providers use ollama qwen3:8b

ainz providers add litellm --preset lite-llm --api-key-env LITELLM_API_KEY
ainz models list litellm --refresh

ainz providers add codex --preset codex --known-model gpt-5.6-sol
ainz providers add claude --preset claude-code --known-model sonnet
```

The config file lives in the platform config directory: `~/Library/Application Support/ainz/config.toml`
on macOS, `~/.config/ainz/config.toml` on Linux. `AINZ_CONFIG` overrides the path, and
`AINZ_MODEL`, `AINZ_ENDPOINT`, `AINZ_PROVIDER`, `AINZ_API_KEY`, `AINZ_MEMORY`, and
`AINZ_SYNAPSE` override values.

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

[memory]
backend = "local"
recall_on_start = true
recall_limit = 5
remember_on_compact = true
teach = false

[synapse]
enabled = false
mesh = false
```

`/settings` opens the same list in a session: provider and model, permissions, memory, Synapse,
the agent mesh, the roster, and header art, each with a line saying what it does.

Without a `provider`, the legacy top-level `endpoint` and `api_key_env` keys describe one HTTP
provider. HTTP profiles use Ainz's own model and tool loop. Process profiles run a coding CLI's
own loop and return its final response as one assistant turn; in `ask` and `read_only` modes
they run read-only, and `auto` permits workspace edits. See
[`docs/providers.md`](docs/providers.md).

## Use

```sh
ainz                                    # interactive
ainz ask "inspect the project and run its tests"
ainz ask --image screenshot.png "explain this interface"
ainz ask --json "summarize this workspace"   # machine-readable event stream
ainz rpc                                # persistent JSON-RPC process

ainz sessions
ainz sessions --search "certificate error"
ainz sessions export SESSION_ID --out session.md
ainz resume
ainz resume SESSION_ID --at NODE_ID "continue from this branch"
ainz skills
ainz skills proposed
ainz prompts
ainz usage
ainz memory list
ainz memory add the staging database is named orbit
ainz synapse
ainz mcp
ainz plugins list
ainz doctor
```

JSON mode never opens an approval prompt; pass `--permissions auto` explicitly when a
noninteractive request may write files or execute commands.

`--yeet` is the blunt version: every tool call runs without asking, and every plugin on disk
loads whether or not it was approved. `/yeet` does the same inside a session. Nothing is
written — not the config file, not the plugin grants — so both end with the process, and the
status line says `yeet` while it is on. Choosing a mode with `/permissions` or in `/settings`
ends it early. It runs code you have not vetted, which is what the
name is for.

In a terminal, Ainz runs a Ratatui interface with a streaming transcript, permission prompts
that show the tool's arguments, tool activity, and a subagent roster. Type `/` to open the
command palette and fuzzy-search commands and prompt templates, and `@` to complete a path in
the workspace.

The prompt is a readline: `Up` and `Down` walk earlier prompts and come back to the line being
written, `Left`/`Right` and `Alt+←`/`Alt+→` move by character and word, `Ctrl+A`/`Ctrl+E` reach
its ends, `Ctrl+U`/`Ctrl+K`/`Ctrl+W` cut, and `Shift+Enter` — or a trailing backslash — adds a
newline. `Esc` twice steps back to the last prompt and puts it in the line to be changed, taking
the session from there. `/vim` turns on modal editing. The wheel, `Shift+↑`/`Shift+↓` and
`PageUp`/`PageDown` scroll the transcript; `Ctrl+O` expands what tools returned in full. The
mouse selects a field or a menu row in the setup screens, and holding `Shift` while dragging
selects text the way it normally would.

`/inline` draws the prompt at the bottom of the terminal's own scroll instead of taking the
whole screen, so finished output stays in the scrollback the terminal already keeps — at the
cost of the roster. It applies at the next launch.

The tools a session has, and what each one takes, are in [`docs/tools.md`](docs/tools.md); what
may run without asking is in [`docs/permissions.md`](docs/permissions.md).

A permission prompt shows what the call would actually do — an edit as a diff, a command as the
command — and takes three answers: `y` allows it once, `n` refuses, and `a` keeps the decision.
`a` writes a standing rule, the tool alone or the tool with the first word of its command, so
`shell(git *)` is a decision you can actually mean; it applies to the run in flight and every one
after. `/rules` lists them, `/rules clear` forgets them, and they live in the config where a
headless run reads the same ones. Deny beats allow.

A long command reports itself while it runs rather than after: the call in the transcript grows
a line showing the last thing it wrote, and `Ctrl+O` opens the whole of it. A run of more than
ten seconds rings the terminal when it finishes. An image pasted or dragged into the prompt
attaches to the next message.

`Ctrl+L` toggles the roster
and remembers the choice, `Ctrl+1` selects the primary transcript, `Ctrl+2` through `Ctrl+9`
select subagents, and `Ctrl+=` / `Ctrl+-` cycle through them; those chords need a terminal
that speaks the kitty keyboard protocol, and `/agent N` works everywhere. `Ctrl+C` cancels an
active run, and a second `Ctrl+C` abandons a run whose provider ignores the cancel. `/settings`
opens the settings screen, `/memory` and `/remember` reach memory, and `/synapse` shows the
integration state. Text
entered during a run is queued as steering for the next safe turn boundary. Redirected input
keeps a plain line interface.

An empty transcript shows one of ten built-in mastheads. Paint your own in the
[masthead studio](https://wess.io/ainz/masthead/), or bring UTF-8 ASCII or ANSI-SGR art, and
drop it in the config directory's `headers/` folder or a project's `.ainz/headers/`.
`/headers` lists them and `/header NAME` selects one; see [`docs/headers.md`](docs/headers.md).

The unit tests cover the model behind the prompt; what a keystroke actually draws needs a
terminal, so `scripts/tui-check.py` drives a real one. It opens a pty of its own, builds a
throwaway workspace with a fake provider, and checks 44 things — history, the cursor keys, `@`
completion, the rewind, the mouse, vim mode, and both ways of drawing, including what happens
when the terminal will not say where the cursor is. It needs `pyte` and never touches the
terminal it is run from.

```sh
pip install pyte
cargo build && python3 scripts/tui-check.py
```

## Hooks

A session crosses a few points nothing else can see: before its first turn, before and after
every tool call, and when a run ends. A hook is a command run at one of those, taking the event
as JSON on stdin. A `pre_tool` hook is the one with a vote — a non-zero exit blocks the call and
its stderr becomes the error the model reads; every other event's exit status is advisory. See
[`docs/hooks.md`](docs/hooks.md).

```toml
[hooks]
post_tool = [{ command = ["cargo", "fmt"], matcher = "edit" }]
```

## Memory

A session that forgets everything at the end re-derives the same things next week. Ainz keeps
durable memories, searches earlier sessions, and can let a session write down a procedure it
worked out.

Memory is on by default and local: Markdown files under the data directory, private to this
machine, one file per memory, scoped to the workspace or global. The newest are recalled into
the system prompt when a session opens, a `memory` tool recalls and stores more, and when the
transcript is compacted the session is asked to write down anything durable it has not stored —
the one moment where not having written something down costs you immediately.

A `sessions` tool searches earlier transcripts by term and returns ids and excerpts, so "we hit
this last week" is recoverable without having remembered it in advance.

With `memory.teach` on, a `learn` tool lets a session propose a skill from something it figured
out, and correct one that turned out wrong. A proposal waits in `skills/proposed/`, which
discovery does not read, until `ainz skills approve NAME`. See [`docs/memory.md`](docs/memory.md).

## Synapse

[Synapse](https://wess.io/synapse/) keeps memory, one skill library, and an agent mesh on your
machine, shared with the other tools you use. Ainz can use all three, and runs the same without
it — the setting has to be on and the binary has to be installed.

```sh
ainz synapse enable
ainz memory backend synapse
ainz synapse mesh on
```

Turning it on registers `synapse mcp` for the session, appends Synapse's guidance and your
`SOUL.md` to the system prompt, and points memory and taught skills at Synapse instead of local
files, so a decision recorded here reaches a later session in Claude Code or Codex. With the mesh
on, the session and every subagent register under their own names and can message each other,
and you have a seat of your own to answer from. See [`docs/synapse.md`](docs/synapse.md).

## Instructions, skills, and prompts

Ainz reads the layouts other harnesses already use, so an existing project works without an
import step or a copy that drifts.

`AGENTS.md` and `CLAUDE.md` are both read at every level from the filesystem root down to the
workspace, nearest last, along with `~/.claude/CLAUDE.md` and the config directory's `AGENTS.md`.

`SKILL.md` directories are discovered up front from `skills/`, `.agents/skills/`,
`.claude/skills/`, and `.ainz/skills/` beside the workspace, plus `~/.claude/skills/` and the
config directory. Only their names and descriptions occupy the prompt; one `skill` tool loads a
skill's text when the model asks for it. A skill that ships scripts or reference files lists them
on load, and the same tool serves them with `{"name": "...", "file": "scripts/run.sh"}`, which is
how a skill outside the workspace reaches its own files.

Markdown prompt templates come from `.claude/commands/` and `.ainz/prompts/` beside the
workspace, plus `~/.claude/commands/` and the config directory. A subdirectory namespaces its
templates, so `commands/api/audit.md` runs as `/api:audit`. Bodies expand `$ARGUMENTS` and `$1`
as well as `{{args}}` and `{{1}}`, and an `argument-hint` in the front matter becomes the usage
shown in the palette.

Nearer definitions win, so a project can override a shared skill or command by name. `ainz
skills`, `ainz prompts`, and `ainz doctor` show what was found.

## Import

Skills, commands, and instruction files are read where they already live, so most of a machine
needs no import step. Tool servers are the exception: MCP configuration is per-tool, and Ainz
starts servers only from its own profile.

```sh
ainz import              # what Claude Code, Codex, Cursor, and the rest have
ainz import --all        # copy everything not already available
ainz import github       # or just this one
```

`/import` opens the same list in a session as a checklist. Anything Ainz already reads is
marked already available and left unticked, so importing twice changes nothing. An entry that
carries a token or password inline is marked too, because importing copies the secret into the
Ainz profile. Skills and prompts come along the same way, from the Synapse library, Codex, and
pi. See [`docs/import.md`](docs/import.md).

## External tools

Stdio servers are registered from the CLI; Streamable HTTP servers are added to the profile
file directly. The profile lives in the user config directory only, so cloning a repository
cannot start a server. A harness such as Synapse may pass an ephemeral launch profile with
`--mcp-config`. Schemas stay out of the model context until it searches for a tool and asks for
one schema. See [`docs/mcp.md`](docs/mcp.md).

```sh
ainz mcp add synapse --required -- /absolute/path/to/synapse mcp
```

## Plugins

Native plugins are discovered from the config directory's `plugins/` folder and from
`.ainz/plugins/*/plugin.toml` between the filesystem root and the workspace. Portable Agent
Plugins 1.0 packages using `plugin.json`, `skills/`, and `mcp.json` are also discovered there
and from `~/.agents/plugins` or `.agents/plugins`. The nearest project definition wins when
names collide.

Discovery reads manifests but never runs anything. Approve the exact content before a plugin's
tools become available:

```sh
ainz plugins list
ainz plugins approve example
ainz plugins revoke example
```

The approval pins the manifest, every file under the plugin directory, and the executable or
component it names; changing any of them returns the plugin to pending, and the artifact is
rechecked when it runs. Component plugins run with fuel, memory, instance, table, and
wall-clock limits, no WASI authority, and capability-checked host imports per tool. Process
plugins are trusted native programs; capabilities drive visibility and approval but cannot
sandbox one. See [`docs/plugins.md`](docs/plugins.md).
