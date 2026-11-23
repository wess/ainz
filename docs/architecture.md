# Architecture

Ainz keeps the orchestration core independent from transports and user interfaces.

1. `Agent<P>` owns the bounded tool loop and accepts any `ChatProvider`. Its tool set, workspace,
   event sink, and approver remain available across runs. Instructions, permission mode, rules,
   limits, and hooks travel in `RunOptions`. An optional `ModelSelector<P>` chooses the provider
   at the start of a run. The chosen provider handles the tool loop and any continuations.
2. `ToolSet` gives built-ins, skills, external servers, subagents, and plugins one async
   interface and one permission path: a call's risk is checked against the standing rules
   first, then the permission mode, then a `pre_tool` hook that can still refuse it. See
   [`docs/tools.md`](tools.md) for the built-ins and [`docs/permissions.md`](permissions.md) for
   that path in full.
3. `Session` is an append-only tree. Checkout moves a cursor; it never destroys another
   branch. Summaries are attached to cursors and apply only to descendant context.
4. `EventSink` decouples streaming from presentation. A tool call that produces output before it
   finishes reports it as a `ToolDelta`, not just a start and an end. The terminal, NDJSON
   output, and JSON-RPC mode consume the same events.
5. `RunController` queues steering at safe conversation boundaries and cancels provider
   calls, compaction, approvals, tools, and hooks without adding UI state to the agent.
   Steering has a bounded queue; cancellation has a separate signal. Closing steering at
   the final reply is atomic with taking the last batch, so an accepted message cannot fall
   between the last turn and the end hooks. Those hooks remain cancellable.
6. The run recovers from replies that did not finish. Which effect comes next is decided by a
   pure step function (`agent/step.rs`): a reply cut off at its output limit leads to a
   continuation request, and three in a row without a successful tool call pause the run. A
   reply that broke off mid-stream is requested again. The context estimate is learned from the
   provider's own token counts. The agent keeps the estimate while it uses that provider.

The CLI's `ModelRouter` resolves the session's fixed model or Auto strategy against its provider
catalog. In Auto the boss receives a bounded task brief and eligible candidates. The selector
validates its decision, emits `ModelSelected`, and passes reported routing usage into the worker's
run budget. `Session` stores the picker selection for resume. Provider adapters replay ordinary
messages and tool results across model switches and scope opaque reasoning to its originating
connection and model. See [provider configuration](providers.md#automatic-model-routing).

The terminal projects `EventSink` events into transcript and subagent views. Each worker
retains its provider, session history, and subagent lifetime while the attached client switches
between live sessions. The client relays terminal bytes and keeps no second runtime copy.

Extensions are discovered without execution. Static manifests define schemas and
capabilities, content-pinned grants decide which extensions load, and runtime adapters
produce ordinary tools. Component instances receive fresh stores and only the host
capabilities declared by the selected tool. Native process plugins are explicitly a
trusted tier.

External tool servers use one lazy dispatcher so large catalogs do not occupy the model
context. Stdio and Streamable HTTP clients share initialization, pagination, result, and
error behavior behind the hub.

Durability is a backend rather than a feature. `MemoryStore` and `Teacher` each have a local
implementation and a Synapse one behind the same interface, so a session's tools, prompts, and
behavior do not change with the choice; only where the writing lands does. The integration is
composed from parts that already exist — Synapse is an optional entry in the same server hub,
reached through the same client — so nothing in the core knows about it. Recall is assembled
into the system prompt at agent construction and marked as context, never as instruction.

Subagents are named and tracked by a registry that the delegation tool owns, which is what lets
one run in the background and be collected by name later. With the mesh on, each child registers
its own client rather than sharing its parent's, so identity on the mesh is per agent and a
failure to register costs that child its seat and nothing else.

Cancelling a collection returns the task to that registry, where it remains addressable.
Dropping the registry aborts the tasks it still owns; hosts should keep it alive for the lifetime
of their background work. Aborting a task remains cooperative at async yield points.

The TUI keeps IRC labels, activity state, topics, and roster placement in its presentation layer.
[termweave](../crates/termweave/readme.md) handles Markdown and Unicode line layout. Header files
are parsed as text and SGR styles; they cannot execute cursor controls or scripts. The browser
studio exports the same ANSI format, and the mascot generator also produces its shared SVG
preview and editable starter so the site and terminal use the same artwork.

See [embedding](embedding.md) for the feature boundary and host responsibilities, and
[quality](quality.md) for limits that remain, including the unbounded convenience event queue.

Commands the model runs can be confined by the operating system. `sandbox::confine` rewraps an
already-built command under `sandbox-exec` or `bwrap` when `AINZ_SANDBOX` asks for it, and
returns it untouched otherwise, so the shell tool, jobs, hooks, plugin commands, and checks each
change by one call. An unavailable sandbox refuses the command; it never falls back to none.

The person's own Lua runs in one long-lived runtime (`script`), made only when there is an
init.lua to run. It never reaches into the interface or the agent. What a script asks for is
queued as effects the interface carries out between frames, and what it draws (panels, status
items, the topic) is read when a frame is drawn. Option writes are recorded as paths and applied
to the configuration as it stands, so a keymap never undoes a change made in `/settings`. The
agent sees scripts only through ordinary tools and a `pre_tool` gate. `Config::settle` records
the configuration once scripts and flags have applied, and a save then writes only what changed
after that. That is how a script's options stay out of `config.toml`.

Named themes live in the optional CLI layer. The catalog reads bounded TOML files and the chat
renderer maps palette roles over the visible terminal buffer. It does not traverse conversation
history or allocate a second runtime. Header regions retain their source colors. See
[themes](themes.md) for the supported roles and surfaces.

## Live terminals

The CLI live host owns a PTY and worker process per session. An attached terminal relays bytes
over an owner-only Unix socket. The worker owns agent state, approvals, and saved history; the
client owns the picker. Bounded messages and queues limit transport memory. Filesystem watches
update live metadata and branch labels without idle polling. Hosting, terminal handling, and
watchers stay behind the CLI feature; the core remains usable without terminal dependencies.
