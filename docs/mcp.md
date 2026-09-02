# External tool servers

AgentX supports the stable `2025-11-25` stdio and Streamable HTTP transports. Persistent
server configuration is read only from the user profile. Repository files cannot add or
start a server. A launcher may supply an additional configuration explicitly with
`--mcp-config`.

## Profile

The simplest way to register a stdio server is through the CLI:

```sh
agentx mcp add files -- /absolute/path/to/server --read-only
```

This writes the platform user profile (`~/Library/Application Support/agentx/mcp.toml`
on macOS). Its conventional MCP shape is:

```toml
[servers.files]
transport = "stdio"
command = "/absolute/path/to/server"
args = ["--read-only"]
enabled = true
required = false
timeout_ms = 30000
```

Streamable HTTP servers use a URL. Sensitive header values come from environment
variables and are never stored in the profile:

```toml
[servers.remote]
transport = "streamable_http"
url = "https://tools.example.test/mcp"
header_env = { Authorization = "TOOLS_AUTHORIZATION" }
enabled = true
required = false
timeout_ms = 30000
```

`required` servers initialize before the first model request and stop startup on
failure. Optional servers start when their catalog is first searched.

Inspect configuration without starting optional servers:

```sh
agentx mcp
agentx mcp --json
agentx mcp remove files
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

[Synapse](https://wess.io/synapse) can connect AgentX itself:

```sh
synapse connect agentx
```

For a checkout of Synapse that predates native AgentX support, register it manually:

```sh
agentx mcp add synapse --required -- /absolute/path/to/synapse mcp
```

AgentX initializes required servers before the first model request and includes their MCP
server instructions in the agent instructions. This lets Synapse provide its memory and mesh
session contract, including startup recall, without AgentX reading Synapse's database or
depending on its private implementation. Synapse-launched agents receive an isolated,
ephemeral `--mcp-config`; each process therefore gets its own mesh identity.

## Context behavior

AgentX exposes one built-in `mcp` tool regardless of catalog size. It has three
operations:

- `search` starts configured servers as needed and returns matching names and summaries
- `schema` loads the full input schema for one selected tool
- `call` invokes that tool and passes through text and structured output

Only the dispatcher schema and server names occupy the base prompt. Calls pass through the
normal permission policy. Synapse's read-only discovery and recall operations are classified
as reads, `remember` is a write, and mesh mutations are executions.

## Lifecycle

The client sends `initialize`, checks the negotiated version, sends
`notifications/initialized`, then uses paginated `tools/list` and `tools/call`. Stdio
uses newline-delimited UTF-8 JSON-RPC and rejects unsupported server-to-client requests
with method-not-found. Streamable HTTP retains negotiated session IDs, accepts JSON or
server-sent event responses, and sends the protocol-version header after initialization.

The implemented transport follows the [protocol transport specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
and [schema reference](https://modelcontextprotocol.io/specification/2025-11-25/schema).
