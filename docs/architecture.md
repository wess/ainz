# Architecture

Ainz keeps the orchestration core independent from transports and user interfaces.

1. `Agent<P>` owns the bounded tool loop and accepts any `ChatProvider`. What the agent is —
   provider, tool set, workspace, event sink, approver — stays fixed for the process; how a run
   behaves — instructions, permission mode, standing rules, step and context limits, hooks —
   travels separately in `RunOptions`, rebuilt whenever configuration changes.
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
   or tool futures without adding UI state to the agent.

The terminal multiplexer is a view and control plane over those primitives. Panes subscribe
to `EventSink`; they never own provider processes, session history, or subagent lifetime.
Subagent rows are projections of durable session IDs and start/end events. This keeps the TUI
replaceable and prevents a pane from becoming a second source of runtime truth.

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
