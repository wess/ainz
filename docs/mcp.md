# External tool servers

Ainz speaks the `2025-11-25` stdio and Streamable HTTP transports. Persistent server
configuration is read only from the user profile, so a cloned repository cannot start a
server on its own. Two explicit paths add more: a launcher may pass `--mcp-config`, and an
approved portable plugin may ship an `mcp.json` (see [plugins](plugins.md)).

Synapse is the exception to the profile rule: turning the integration on registers its server for
the session without writing anything, and turning it off removes it. See [Synapse](synapse.md).

## Profile

Register a stdio server through the CLI:

```sh
ainz mcp add files -- /absolute/path/to/server --read-only
```

This writes the profile in the platform config directory (`~/Library/Application
Support/ainz/mcp.toml` on macOS, `~/.config/ainz/mcp.toml` on Linux; `AINZ_MCP_PROFILE`
overrides the path). The file is created with mode `0600`. Its shape:

```toml
[servers.files]
transport = "stdio"
command = "/absolute/path/to/server"
args = ["--read-only"]
enabled = true
required = false
timeout_ms = 30000
```

Streamable HTTP servers are added to the file directly. Put credentials in `header_env`, which
names an environment variable, rather than `headers`, which stores the value in the profile:

```toml
[servers.remote]
transport = "streamable_http"
url = "https://tools.example.test/mcp"
header_env = { Authorization = "TOOLS_AUTHORIZATION" }
enabled = true
required = false
timeout_ms = 30000
```

Server names use letters, digits, `.`, `_`, and `-`. `ainz mcp --json` prints the profile with
`headers` and `env` values redacted.

`required` servers start together before the first model request and stop startup on failure.
Optional servers start when the catalog is first searched or when a call names them. An optional
server that fails to start is skipped by later searches until something names it directly. A
server that stops answering, closes its pipe, or times out is dropped and restarted on the next
call; the request that hit the failure reports it.

```sh
ainz mcp
ainz mcp --json
ainz mcp remove files
```

`--mcp-config PATH` accepts the JSON launch format used by MCP-aware harnesses:

```json
{
  "mcpServers": {
    "synapse": {
      "command": "/absolute/path/to/synapse",
      "args": ["mcp"],
      "env": { "SYNAPSE_PROJECT_DIR": "/absolute/path/to/project" }
    }
  }
}
```

These launch-only servers are merged in memory, treated as required, and never copied into
the user profile.

## Synapse

[Synapse](https://wess.io/synapse) can connect Ainz itself:

```sh
synapse connect ainz
```

For a checkout of Synapse that predates native Ainz support, register it manually:

```sh
ainz mcp add synapse --required -- /absolute/path/to/synapse mcp
```

Required servers' MCP instructions are appended to the agent instructions, labelled with the
server name and capped at 4 KiB each. This lets Synapse provide its memory and mesh session
contract, including startup recall, without Ainz reading Synapse's database. Synapse-launched
agents receive an isolated, ephemeral `--mcp-config`, so each process gets its own mesh identity.

## Context behavior

Ainz exposes one built-in `mcp` tool regardless of catalog size, with three operations:

- `search` starts configured servers as needed and returns matching names and summaries
- `schema` loads the full input schema for one selected tool
- `call` invokes that tool and passes through text and structured output

Only the dispatcher schema and server names occupy the base prompt. Calls pass through the
normal permission policy: a tool whose `tools/list` entry carries `readOnlyHint` is a read,
one with `destructiveHint = false` is a write, and everything else is an execution that `ask`
mode confirms. A tool the server has not described yet counts as an execution.

## Lifecycle

The client sends `initialize`, checks the negotiated version, sends
`notifications/initialized`, then uses paginated `tools/list` and `tools/call`. Stdio uses
newline-delimited UTF-8 JSON-RPC, skips non-JSON lines such as package-runner banners, caps a
line at 16 MiB, and answers server-to-client requests with method-not-found. Streamable HTTP
keeps the negotiated session ID, sends the protocol-version header after initialization,
refuses redirects so headers never replay to another origin, accepts JSON or server-sent
event bodies up to 8 MiB, and returns as soon as the event answering the request arrives.

The implemented transport follows the [protocol transport specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
and [schema reference](https://modelcontextprotocol.io/specification/2025-11-25/schema).
