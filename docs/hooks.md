# Hooks

A session crosses a few fixed points nothing else in Ainz can see: before its first turn, before
and after every tool call, and when a run ends. A hook is a command run at one of those points,
the way a git hook runs on commit.

## Configuration

```toml
[hooks]
session_start = [
  { command = ["notify-send", "ainz session started"] },
]
pre_tool = [
  { command = ["./scripts/guard-writes.sh"], matcher = "write" },
]
post_tool = [
  { command = ["./scripts/log-tool.sh"] },
]
session_end = [
  { command = ["./scripts/wrap-up.sh"] },
]
```

Each event holds a list of hook definitions, run in order. `command` is an argv, run directly and
never through a shell — no quoting to get wrong, no `$PATH` surprises. `matcher` is optional and
only means anything for `pre_tool`/`post_tool`: with none, a hook runs for every tool call; with
one, it runs only when the tool's name contains it (`"write"` matches `write` and `overwrite`), or
matches it as a glob if the pattern has a `*` (`"shell*"` matches `shell` but not `background_shell`).
`session_start` and `session_end` ignore `matcher` — there is no tool to match against.

An old config with no `[hooks]` table loads exactly as before; the section is empty by default.

## The payload

Every hook gets one JSON object on stdin, nothing on its command line or in the environment:

```json
{"event": "pre_tool", "workspace": "/path/to/project", "session_id": "01988c2f-2b3a-...",
 "tool": {"name": "write", "arguments": {"path": "config/prod.env", "content": "..."}}}
```

`workspace` and `session_id` are always present. The rest only appears where it applies:

| event | `tool` | `output` | `error` |
|---|---|---|---|
| `session_start` | | | |
| `pre_tool` | name and arguments | | |
| `post_tool` | name and arguments | what the tool returned | whether it failed |
| `session_end` | | the final message, if the run succeeded | whether the run failed |

## Blocking

`pre_tool` is the one event with a vote: a hook that exits non-zero, times out, or fails to start
blocks the call, and the tool returns an error quoting the hook's stderr instead of running.
Every other event is advisory — a failing `session_start`, `post_tool`, or `session_end` hook is
reported and the session carries on, the same way a slow linter does not stop a save.

A hook gets ten seconds. Past that it is killed and treated as a failure — blocking for
`pre_tool`, reported for everything else — because a hook that never returns must never be able
to hang the session waiting for it.

## Example: keep writes inside the sanctioned directories

```toml
[hooks]
pre_tool = [
  { command = ["./scripts/guard-writes.sh"], matcher = "write" },
]
```

```sh
#!/bin/sh
# scripts/guard-writes.sh
path=$(cat | jq -r '.tool.arguments.path // empty')
case "$path" in
  src/* | tests/*) exit 0 ;;
  *) echo "writes are only allowed under src/ and tests/, not $path" >&2; exit 1 ;;
esac
```

A call to `write` with `path = "config/prod.env"` comes back to the model as `blocked by
pre_tool hook ./scripts/guard-writes.sh: writes are only allowed under src/ and tests/, not
config/prod.env` instead of touching the file.
