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

The other methods are `state`, `new_session`, `save`, and `shutdown`. Risky tools are
denied unless the process was started with `--permissions auto`; RPC mode never opens an
interactive approval prompt. Sessions save after each completed or cancelled turn unless
`--no-save` is supplied.

A hosted session is built like any other, so [memory](memory.md) and the
[Synapse](synapse.md) integration apply here too: recalled memories reach the system prompt
before the first turn, and remembering is a tool call the host sees as an ordinary `event`.
Storing a memory is a write, so it needs `--permissions auto` for the same reason every other
write does.
