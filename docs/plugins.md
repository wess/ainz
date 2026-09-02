# Plugins

AgentX supports two complementary formats:

- Native `plugin.toml` packages expose trusted process tools or capability-scoped WebAssembly
  components through the AgentX API described below.
- Portable [Agent Plugins 1.0](https://agent-plugins.org/specification) packages use a root
  `plugin.json` plus the standard `skills/` and `mcp.json` components shared by compatible agent
  clients.

Portable packages are discovered from the normal AgentX plugin roots and from
`~/.agents/plugins/*/plugin.json` or project `.agents/plugins/*/plugin.json`. This makes a package
usable by AgentX while remaining installable by clients such as Pi. MCP servers are namespaced as
`<plugin>__<server>`, and stdio servers receive `PLUGIN_ROOT` plus a dedicated writable
`PLUGIN_DATA` directory. AgentX expands those variables in arguments, environment values, and the
working directory as required by the portable specification.

Both formats use content-pinned approval. For portable packages, the fingerprint covers every
regular file beneath the plugin root, including bundled MCP executables and skill resources;
changing any package content returns it to pending.

A plugin is a directory containing `plugin.toml` and either a sandboxed component or a
trusted process. Static schemas are read from the manifest without starting the runtime.

The architecture has three deliberately separate layers:

1. The catalog discovers scoped manifests, validates them, and resolves name shadowing.
2. The grant store pins approval to the manifest and runtime artifact bytes.
3. Runtime adapters turn approved declarations into the same `Tool` interface used by
   built-ins. Adding another runtime does not change the agent loop.

Plugin directories closer to the active workspace replace global or parent definitions
with the same plugin name. Tools are still checked for collisions when the final tool set
is assembled, so an extension cannot silently replace a built-in or another plugin.

## Manifest

```toml
capabilities = ["workspace_read"]

[plugin]
name = "example"
version = "0.1.0"
api = 1

[runtime]
kind = "process"
command = ["bin/example"]
timeout_ms = 30000

[[tools]]
name = "inspect"
description = "Inspect a project artifact"
capabilities = ["workspace_read"]
parameters = { type = "object", properties = { path = { type = "string" } }, required = ["path"] }
```

Names use lowercase letters, numbers, and underscores. Tools are exposed as
`<plugin>_<tool>`, so the example above registers `example_inspect`.
Tool names must be unique inside a plugin, descriptions must be non-empty, and
`parameters` must be a JSON Schema object.

Plugin capabilities are:

- `compute`
- `workspace_read`
- `workspace_write`
- `process_exec`
- `network`

Every tool must declare a non-empty subset of the plugin's capabilities. The highest
risk capability on that tool determines whether a call needs approval.

## Component runtime

Use a component when host authority should be explicit and capability-scoped:

```toml
capabilities = ["compute"]

[runtime]
kind = "component"
path = "plugin.wasm"
timeout_ms = 30000
memory_bytes = 67108864
fuel = 10000000
```

Components implement [`wit/plugin.wit`](../wit/plugin.wit). They receive the selected
tool name and JSON arguments, then return either a string result or a string error.
Every invocation gets a fresh store with bounded fuel, memory, tables, instances, and
wall-clock time. The WASI context has no inherited environment, filesystem, network,
arguments, or standard streams.

The imported `agentx:plugin/host` interface exposes `read-file`, `write-file`, `run`,
and `fetch`. Each function checks the capabilities declared by the selected tool; one
tool cannot borrow authority declared for another tool in the same component. Transfers
are bounded by the configured tool-output limit, workspace paths cannot escape through
absolute paths, parent traversal, or symlinks, process output is drained under the
runtime timeout, and network access accepts only HTTP and HTTPS URLs.

## Process runtime

Relative executables resolve from the directory containing `plugin.toml`. The process
runs with the active workspace as its current directory.

## Request

AgentX sends one newline-terminated object:

```json
{
  "version": 1,
  "id": "019...",
  "method": "tool.call",
  "params": {
    "name": "inspect",
    "arguments": { "path": "src" },
    "context": { "workspace": "/absolute/workspace" }
  }
}
```

## Response

Return either a result:

```json
{"result":{"files":12}}
```

or an error:

```json
{"error":"artifact not found"}
```

The response must occupy one line. AgentX enforces the manifest timeout, drains stdout
and stderr concurrently with hard capture bounds, and truncates the result to the
configured tool-output limit.

## Trust model

`agentx plugins approve <name>` records a SHA-256 digest covering both `plugin.toml` and
the referenced component or process executable. Changing either returns the plugin to
pending. Process commands must therefore name a relative or absolute executable file,
not rely on shell path lookup.
