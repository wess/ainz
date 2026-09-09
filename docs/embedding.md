# Embedding

Ainz's Rust library runs without a terminal or a background service. The host supplies a
provider, tools, workspace, event callback, approval callback, and session. Constructing an
`Agent` does not load user configuration, discover extensions, read credentials, or save state.

Until the core has a registry release, use a checkout or a pinned Git revision:

```toml
[dependencies]
ainz = { path = "../ainz", default-features = false }
```

## Features

| Features | Includes |
| --- | --- |
| none | Agent loop, provider adapters, tools, sessions, controls, memory, MCP, process plugins |
| `cli` | CLI binary, terminal UI, headers, themes, Markdown rendering |
| `lua` | Sandboxed Lua plugin execution |
| `wasm` | WebAssembly component plugin execution |
| default | `cli`, `lua`, `wasm` |

Each feature is independent. A library host can enable Lua without the CLI or WebAssembly.
The catalog still discovers disabled runtime kinds and checks their content fingerprints;
attempting to load an approved plugin for an unavailable runtime returns a feature-specific
error. It never executes it through a fallback runtime.

For a CLI with Lua and process plugins:

```sh
cargo build --release --no-default-features --features cli,lua
```

The default CLI build retains all runtimes. Feature removal changes dependencies and available
capabilities; it does not change a tool's permissions. The core still targets native systems
with Tokio and filesystem/process support. Browser WebAssembly, a C ABI, and language bindings
are not provided by this feature split.

## Host contract

```rust,no_run
use ainz::{Agent, Event, EventSink, RunOptions, Session, deny_all};
use ainz::tool::ToolSet;

async fn run(provider: impl ainz::ChatProvider) -> anyhow::Result<String> {
  let workspace = std::env::current_dir()?;
  let events = EventSink::new(|event| {
    if let Event::TextDelta { text } = event {
      print!("{text}");
    }
  });
  let agent = Agent::new(provider, ToolSet::default(), workspace.clone(), events, deny_all());
  let mut session = Session::new(workspace);
  let options = RunOptions {
    instructions: "Answer the user's request.".into(),
    ..Default::default()
  };
  agent.run(&mut session, "Hello".into(), options).await
}
```

An empty `ToolSet` grants no tools. Add individual `Tool` implementations or explicitly opt into
`tool::builtins()`. `RunOptions::default()` uses ask-mode permissions and bounded step, context,
and tool-output settings. `deny_all()` refuses calls that require approval; read-risk calls can
still run. `RunOptions::from(&config)` copies limits, rules, and hooks without loading anything.
Instructions and memory guidance remain explicit host inputs. Invalid run options fail before
the session is changed or hooks/providers are started.

The event callback is synchronous: keep it short. It runs on the provider/tool's execution
path. `EventSink::channel()` is an unbounded convenience queue, so a host handling large output
must drain it promptly or supply its own callback and retention policy. A slow callback can
delay cancellation. Host tools must cooperate with async cancellation and own their child
process cleanup.

For steering and cancellation, create a fresh `run_control()` pair for each run and pass its
inbox to `run_controlled()`. Cancellation is sticky and completed runs close their steering queue.
Keep the controller in the host interface. A cancellation request is not a completion receipt:
await the run future before reporting the final result. `TurnEnd` is emitted after the end
hooks finish; host persistence is still separate. Publish completion after the future returns
and the host's save succeeds. Cancellation interrupts provider calls, compaction, approvals,
tools, and every hook phase. Remaining hooks are skipped on cancellation. Completed tool
results and reported usage survive interruption; unfinished tool calls receive cancellation
results so the transcript can be resumed.

Steering accepts up to 32 queued messages, each at most 64 KiB of UTF-8. Empty/oversized
messages and full, cancelled, or closed queues return `false` from `steer()`; `try_steer()`
returns the specific rejection reason. Preserve the
host's draft when rejected. Messages stay in the bounded queue until a safe turn boundary;
cancellation uses a separate signal and takes priority. Dropping the controller detaches
control without cancelling the run. Once the final reply has no pending steering, the queue
closes before end hooks start. New steering is then rejected while cancellation stays available.

Cancelling `SubagentRegistry::collect()` keeps its task available for another collection.
Dropping the registry aborts its remaining tasks. Keep the registry alive when background work
must survive a frontend being replaced; arbitrary detached work needs a separate durable host.

`Session` is serializable. The host can store it directly or use `SessionStore::new(path)`.
Saving and resuming do not snapshot workspace files. A run can mutate its session before
returning an error, so preserve that session for inspection or retry. Each file save uses a
unique temporary file, syncs the data, atomically replaces the checkpoint, and syncs the parent
directory. Concurrent saves cannot mix their files, but the last writer wins: serialize writers
when preserving every update matters. A crash can leave temporary files, which discovery
ignores. A save interrupted after replacement may already have committed; reload to inspect it.

## Runnable example

[`examples/embedded.rs`](../examples/embedded.rs) supplies a local provider and custom read tool,
streams a response, serializes a checkpoint, and resumes it. It requires no credentials or
network calls and creates no files.

```sh
cargo run --no-default-features --example embedded
# a separate package verifies the public API without workspace feature unification
cargo run --manifest-path tests/fixtures/embedding/Cargo.toml
```

The host can replace the example provider with `HttpProvider`, `ProcessProvider`, or another
implementation of `ChatProvider`. The CLI's automatic assembly of skills, memory, plugins,
and subagents remains in the application layer; embedding currently composes those primitives
explicitly.
