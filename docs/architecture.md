# Architecture

AgentX keeps the orchestration core independent from transports and user interfaces.

1. `Agent<P>` owns the bounded tool loop and accepts any `ChatProvider`.
2. `ToolSet` gives built-ins, skills, external servers, subagents, and plugins one async
   interface and one permission path.
3. `Session` is an append-only tree. Checkout moves a cursor; it never destroys another
   branch. Summaries are attached to cursors and apply only to descendant context.
4. `EventSink` decouples streaming from presentation. The terminal, NDJSON output, and
   JSON-RPC mode consume the same events.
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
