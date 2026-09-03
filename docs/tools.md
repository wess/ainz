# Tools

Every tool call carries a `Risk`: `read`, `write`, `execute`, or `network`. A tool decides its
own risk, sometimes per call — `memory` is a read only for `recall`, `job` is an execution only
for `start` and `stop`, an `mcp` call inherits the risk the server described for that tool. Risk
is what [`docs/permissions.md`](permissions.md) checks against the standing rules and the
permission mode; it has nothing to do with what the tool returns.

This page covers Ainz's own tools. A process provider — a coding CLI run as the model — uses its
own tools instead and never sees these; see [`docs/providers.md`](providers.md).

`read`, `list`, `search`, `write`, and `edit` all take a `path` resolved against the workspace:
an absolute path or a `..` segment is rejected, and the resolved path — following any symlink —
must still land inside the workspace. `write` and `edit` accept a path that does not exist yet,
as long as its nearest existing ancestor is inside the workspace.

A tool that runs long enough to have something to say before it finishes can report that through
`ToolContext::report`, which the agent turns into a `ToolDelta` event carrying the call's id and
the text produced so far. The terminal shows the last non-empty line under the call while it is
still running, instead of nothing until the call ends. Only `shell` does this today — it reports
each line of output as it arrives — but the mechanism is there for any tool.

## Built in

**`read`** — `path` (required), `offset` and `limit` (1-based line number and line count). Streams
the file and returns only the requested window, so a large file costs the window, not the whole
file. Risk: read.

**`list`** — `path` (default `.`). Lists a directory's entries, one per line, sorted, directories
suffixed with `/`. Risk: read.

**`search`** — `query` (a regular expression, required), `path` (default `.`), `max_results`
(default 100, max 500). Shells out to `rg --line-number -e QUERY -- PATH`; ripgrep must be
installed. Risk: read.

**`write`** — `path` and `content` (both required). Creates the file's parent directories if
needed and writes it, creating or replacing it whole. Returns how many bytes were written. Risk:
write.

**`edit`** — `path`, `old`, and `new` (all required). `old` must occur in the file exactly once;
that occurrence is replaced with `new`. Zero matches or more than one is an error rather than a
guess. Risk: write.

**`shell`** — `command` (required), `timeout_ms` (default 30000, 100–300000). Runs `sh -c
COMMAND` in the workspace, in its own process group so a timeout takes the whole tree down.
Stdout and stderr are drained as they arrive and interleaved in arrival order, each line reported
as a `ToolDelta`; the reply ends with `[exit N]`. Risk: execute.

**`fetch`** — `url` (required), `max_bytes` (default: the configured tool-output limit). A GET
over `http(s)` only; it refuses `localhost`, loopback, and the private and link-local ranges
(`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`), so a session cannot use it
to reach the machine's own network or a cloud metadata endpoint. An HTML response is reduced to
text a person would read — tags stripped, `script`/`style` bodies dropped, block tags become line
breaks, entities unescaped; anything else comes back as-is. The reply is the final URL (after
redirects) and content type, then the body. Risk: network — the only built-in tool at that level.

**`todo`** — `action` (`set`, `start`, `done`, or `list`), `items` (for `set`), `target` (for
`start`/`done`, an item's 1-based position or its exact text). A short plan kept in memory for
the session only; nothing here is written to disk. `set` replaces the whole list; `start` marks
one item doing and un-starts whatever was doing before, so only one item is ever in progress;
`done` marks one item done. Every action returns the whole rendered list. Registered alongside
the tools above in `src/app.rs` rather than returned with them, since it is built from session
state instead of being stateless. Risk: read — it changes state kept for this session only,
nothing outside it.

## From other subsystems

**`memory`** — `recall`, `remember`, `forget` durable memories. Risk: read for `recall`, write
otherwise. See [`docs/memory.md`](memory.md).

**`sessions`** — search earlier sessions in this workspace by term and get back ids with
excerpts. Risk: read. See [`docs/memory.md`](memory.md).

**`learn`** — `teach` writes a procedure the session worked out as a skill, `revise` corrects an
installed one. Only appears with `memory.teach` on. Risk: write. See
[`docs/memory.md`](memory.md).

**`mcp`** — `search`, `schema`, `call` against every configured external tool server behind one
dispatcher, so a large catalog costs the model only the dispatcher's own schema. Risk: read for
`search`/`schema`; a `call` inherits the risk the target server described for that tool. See
[`docs/mcp.md`](mcp.md).

**`skill`** — load one skill's instructions on demand, or a file bundled beside it, by name. Risk:
read. Nothing else documents the skill catalog today; this is it.

**`job`** — `start`, `list`, `status`, `output`, `stop` a durable background shell job, so a
command that outlives one turn can still be checked on later. Risk: execute for `start`/`stop`,
read otherwise. Nothing else documents this tool today; this is it.

**`subagent`** — `delegate` runs a focused task in a durable child session and returns its
answer; add `background` to keep working while it runs, then `collect` it by name; `list` shows
what is still running. Always risk: execute. `src/app.rs` builds the delegation closure that
backs it; see [`docs/architecture.md`](architecture.md) for how a subagent's session and events
relate to its parent's, and [`docs/synapse.md`](synapse.md) for what changes when the mesh is on.
