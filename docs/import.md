# Import

Ainz reads the layouts other harnesses already use, so most of what you have works without an
import step: `AGENTS.md` and `CLAUDE.md`, `.claude/skills/` and `~/.claude/skills/`,
`.claude/commands/`, `.agents/` — all discovered in place, no copy to drift.

What it cannot read in place is a **tool server**. MCP configuration is per-tool and lives in
each tool's own file, and Ainz only starts servers from its own profile, so a server you set up
for Claude Code or Codex is invisible to it. That is what import is for.

```sh
ainz import                 # what is available, and where it came from
ainz import --json
ainz import github files    # copy these
ainz import --all           # copy everything not already available
ainz import --kind mcp --all
```

In a session, `/import` opens the same list as a checklist: space selects, `a` selects all or
none, enter copies. `/mcp import` is the same screen.

## Where it looks

| Source | File |
|---|---|
| Claude Code | `~/.claude.json`, global servers plus this workspace's own |
| Claude Desktop | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Codex | `~/.codex/config.toml` |
| Cursor | `~/.cursor/mcp.json` and the workspace's |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Gemini CLI | `~/.gemini/settings.json` |
| This workspace | `.mcp.json`, `.vscode/mcp.json` |
| Skills | the Synapse library, `~/.codex/skills`, `~/.pi/skills` |
| Prompts | `~/.codex/prompts` |
| Memories | Claude Code's memory files for this workspace |

Anything Ainz already reads is listed as **already available** and left unticked, so importing
twice changes nothing. Skills and prompts under `~/.claude` or `~/.agents` are not offered at
all — they are already in the catalog.

## What copying means

A tool server is written into the Ainz MCP profile with `required = false`, so an imported
server that will not start costs you its tools and never a session. Other harnesses distinguish
SSE from Streamable HTTP; Ainz speaks one HTTP transport and imports both as that.

Skills are copied as whole directories into the config directory's `skills/`, scripts and
reference files included. Prompts become `prompts/NAME.md`. Memories are stored through
whichever [memory](memory.md) backend is active.

## Credentials

Some tools keep a token, key, or password inline in their MCP configuration rather than naming
an environment variable. Importing one copies the secret into the Ainz profile, so those rows
are marked **carries credentials** in both the list and the screen. The profile is written
`0600`, the same as the file it came from, but a second copy of a secret is still a second copy:
prefer moving it to `header_env` afterwards, which names an environment variable instead of
holding the value.

```toml
[servers.github]
transport = "streamable_http"
url = "https://api.githubcopilot.com/mcp/"

[servers.github.header_env]
Authorization = "GITHUB_MCP_TOKEN"
```
