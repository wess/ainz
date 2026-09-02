# Plugins

AgentX supports two complementary formats:

- Native `plugin.toml` packages expose trusted process tools or capability-scoped WebAssembly
  components through the AgentX API described below.
- Portable [Agent Plugins 1.0](https://agent-plugins.org/specification) packages use a root
  `plugin.json` plus the standard `skills/` and `mcp.json` components shared by compatible agent
  clients.

Portable packages are discovered from the normal AgentX plugin roots and from
`~/.agents/plugins/*/plugin.json` or project `.agents/plugins/*/plugin.json`. Their MCP servers
are namespaced as `<plugin>__<server>`, and stdio servers receive `PLUGIN_ROOT` plus a dedicated
writable `PLUGIN_DATA` directory. AgentX expands those variables in arguments, environment
values, and the working directory as the portable specification requires. A broken `mcp.json`
in an approved package is an error at startup, not a silent omission.

Static schemas are read from the manifest without starting the runtime. Plugin directories
closer to the active workspace replace global or parent definitions with the same plugin
name. Tools are still checked for collisions when the final tool set is assembled, so an
extension cannot silently replace a built-in or another plugin. A directory whose manifest
fails to parse or validate is reported by `agentx plugins list` and skipped; it never blocks
the other plugins.

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
`parameters` must be a JSON Schema object. `[plugin] enabled = false` keeps a plugin listed
but never loads its tools. `timeout_ms` must be between 1 and 300000; `memory_bytes` and
`fuel` apply to components only and `command` to processes only.

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

Components implement [`wit/plugin.wit`](../wit/plugin.wit); `fixtures/echo` is a minimal
example, built with `cargo build --release --target wasm32-wasip2` and copied to
`tests/fixtures/echo.wasm`. They receive the selected
tool name and JSON arguments, then return either a string result or a string error.
Every invocation gets a fresh store with bounded fuel, tables, instances, and wall-clock
time; `memory_bytes` bounds each linear memory, of which a component may have up to four.
A guest that spins without calling the host is still interrupted: the engine's epoch
ticker makes it yield every 50 ms so the timeout can fire. `memory_bytes` may not exceed
1 GiB. The WASI context has no inherited environment, filesystem, network, arguments, or
standard streams.

The imported `agentx:plugin/host` interface exposes `read-file`, `write-file`, `run`,
and `fetch`. Each function checks the capabilities declared by the selected tool; one
tool cannot borrow authority declared for another tool in the same component. Transfers
are bounded by the configured tool-output limit, workspace paths cannot escape through
absolute paths, parent traversal, or symlinks, process output is drained under the
runtime timeout, and network access accepts only HTTP and HTTPS URLs. `run` is full user
authority: it executes `sh -c` in the workspace with the host's environment, in its own
process group so a timeout takes the whole tree down.

## Process runtime

Relative executables resolve from the directory containing `plugin.toml`. The process
runs with the active workspace as its current directory, in its own process group, with
the host's environment. Its stdin is fed while stdout and stderr drain, so a chatty
program cannot deadlock on a full pipe.

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

`agentx plugins approve <name>` prints what is being trusted (kind, artifact, capabilities,
directory) and records a SHA-256 fingerprint covering the manifest, every regular file and
symlink target beneath the plugin directory, and the referenced component or process
executable, which may live outside the directory. Changing any of them returns the plugin to
pending. The executable or component is hashed again when it runs: a component is compiled
from the bytes that matched, and a process program that no longer matches is refused. The
recheck covers argv[0] only; scripts a command passes as arguments are pinned by the
directory digest at discovery. Process commands must name a relative or absolute
executable file, not rely on shell path lookup, and artifacts must be regular files under
256 MiB.

`agentx plugins revoke <name>` removes the grant. Grants are keyed by plugin name, so
approving a same-named plugin in another workspace replaces the earlier pin.
