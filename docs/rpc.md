# JSON-RPC mode

`ainz rpc` runs a persistent newline-delimited JSON-RPC 2.0 process on stdin and
stdout. It keeps one session and one initialized tool catalog alive. Model and tool
events are emitted as `event` notifications.

Start a turn:

```json
{"jsonrpc":"2.0","id":1,"method":"prompt","params":{"prompt":"inspect the project","images":[]}}
```

While that request is active, queue a steering message or cancel it:

```json
{"jsonrpc":"2.0","id":2,"method":"steer","params":{"message":"focus on tests"}}
{"jsonrpc":"2.0","id":3,"method":"cancel"}
```

Steering is bounded to 32 queued messages of at most 64 KiB each. Rejected messages return an
error instead of a queued acknowledgement. Cancellation bypasses this queue, including during
compaction, approvals, and hooks. Its acknowledgement reports the request; wait for the prompt's
response to know the run has finished. The `turn_end` notification follows the end hooks, while
the prompt response follows session persistence.

The other methods are `state`, `new_session`, `save`, and `shutdown`. RPC mode never opens an
interactive approval prompt, so a risky tool is denied unless the process was started with
`--permissions auto` or the call matches a standing rule in `[rules] allow` — those are checked
before anyone would be asked, on this surface the same as any other. See
[`docs/permissions.md`](permissions.md). Sessions save after each completed or cancelled turn
unless `--no-save` is supplied.

A hosted session is built like any other, so [memory](memory.md) and the
[Synapse](synapse.md) integration apply here too: recalled memories reach the system prompt
before the first turn, and remembering is a tool call the host sees as an ordinary `event`.
Storing a memory is a write, so it needs `--permissions auto`, or a standing rule that covers
it, for the same reason every other write does.

## Event consumers

`text_delta`, `tool_start`, `tool_delta`, and `tool_end` describe incremental output. A
`subagent_start` event includes the child `session_id`, `parent_id`, `name`, and `task`.
`subagent_event` wraps that child's events. The task field lets a host label an agent's buffer
without an extra model request. `subagent_end` records its outcome.

A final text chunk is not a completed run. Keep the UI active through tool and hook work, and
use the prompt response as the completion boundary for an RPC request. For a Rust host that
wants these primitives directly, see [embedding](embedding.md).
