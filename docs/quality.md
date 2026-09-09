# Quality targets

Ainz should be a dependable daily driver with an embeddable native runtime and a compact IRC
interface. Capability claims need task results and failure recovery evidence. Having a tool,
command, or protocol is only the starting point.

This is the working baseline from the code audit on 2026-09-09. It is not a claim of parity or
a completed performance evaluation.

## Current baseline

| Area | Present | Next evidence needed |
| --- | --- | --- |
| Coding loop | Workspace tools, shell, permissions, provider retries, context compaction | Held-out tasks scored for correct changes, verification, retries, tokens, and elapsed time |
| Terminal | IRC rows, terminal Markdown, streaming tools, persistent completion/cancel/failure indicators | Large histories, output floods, resize/scroll latency, keyboard responsiveness |
| Recovery | Branching sessions, atomic synced checkpoints, cancellation through compaction/approvals/tools/hooks, retained usage and completed tool results | Crash recovery during a tool, forced task-abort recovery, save failure recovery in each surface |
| Embedding | Public provider/tool interfaces, host events and sessions, independent CLI/Lua/Wasm features | Stable lifecycle envelope, bounded event delivery, documented compatibility policy |
| Extensions | Content-pinned plugins, lazy MCP tool catalog, skills, hooks, subagents | Hung/disconnected extension soak tests and resource accounting per child |
| Memory | Local and shared memory, prior-session search, proposed reusable skills | Retrieval usefulness, stale memory correction, prompt budget measurements |
| Unattended work | Durable shell jobs and session files | Detached agent supervisor, reconnect/replay, recurring schedules, restart policy |
| Review | File edits and conversation rewind | Change inspector and file checkpoints with conflict-aware restore |
| Providers | Streaming HTTP and process adapters | Explicit capability negotiation and provider-specific conformance fixtures |

## Work order

1. **Runtime reliability and resource bounds.** Introduce a run identity and explicit lifecycle
   covering provider calls, tools, approvals, hooks, compaction, saving, and child shutdown.
   Bound event queues without losing terminal events or letting cancellation sit behind an output
   flood. Steering now has a bounded queue and a separate cancellation signal. Saves now use
   unique temporary files and sync before/after atomic replacement; concurrent updates still need
   host serialization when last-writer-wins is insufficient. Extend failure injection to process
   crashes, external task aborts, failed saves, and child shutdown.
2. **Fast, legible terminal sessions.** Preserve timestamp + `<you>` / `<Ainz>` + text on one
   line, hanging wraps, and an unmistakable ending. Coalesce streaming paints, measure Markdown
   parsing cost, and render/cache only what the viewport needs. Keep full output inspectable
   through bounded memory and on-disk history. Prioritize keyboard response under load.
3. **Coding workflow quality.** Add diff inspection and recoverable edits, then a task evaluation
   suite. Exercise small fixes, multi-file changes, failing tests, interrupted commands, provider
   failures, and resuming work after compaction. Separate harness failures from model failures.
4. **Unattended runtime and adapters.** Put persistence, scheduling, and reconnect in an optional
   host over the core. Keep the terminal a client. Add editor/language bindings and additional
   communication surfaces against that same lifecycle once it is reliable.

The current foundation makes CLI and plugin runtimes optional, adds a standalone embedding
consumer, validates host run options before side effects, and fixes duplicate tool registration
so an error preserves the original tool. It does not complete the work above.

The reliability pass adds cancellation across hooks and compaction, completion after end hooks,
usage retention on failure, completed tool-result retention during post-tool cancellation,
bounded steering and hook output, workspace-relative hooks, and atomic synced session saves.
Regression tests cover all four hook phases, descendant cleanup, full steering queues, pipe
saturation, provider/compaction failure, concurrent saves, and failed replacement cleanup.
Cancelled subagent collection now returns ownership to the registry, and dropping that registry
aborts its remaining tasks. Steering closes atomically at the final reply so messages arriving
during end hooks are rejected with the draft retained, rather than acknowledged and lost.

## Resource measurements

Measure release artifacts on named hardware. Record the OS, target, features, workload, first
launch, warm median/p95, CPU, RSS, and binary size. Separate provider/tool subprocesses from
the harness. Never compare an unoptimized binary to another project's release download.

```sh
cargo build --release --no-default-features --example embedded
python3 scripts/resources.py target/release/examples/embedded --embedded
cargo build --release --no-default-features --features cli,lua
python3 scripts/resources.py target/release/ainz
```

The script's CLI case measures `--version`; it does not measure interactive readiness or a
model-backed task. The embedded case measures two deterministic tool turns and checkpoint
restore. The first launch is reported separately and is not asserted to be a cold disk-cache
measurement. These are reproducible baselines, not end-to-end coding benchmarks.

On this checkout's native dependency graph, removing defaults reduces distinct normal/build
dependencies from 260 to 127; `cli,lua` uses 202. The core excludes the terminal, Markdown, Lua,
and component engines. Dependency counts are build inputs, not a proxy for resident memory.

Release baseline on Apple M1 Max, 64 GiB RAM, macOS 26.6.2 arm64, Rust 1.95.0, 2026-09-09.
Each row uses 25 launches; warm statistics exclude the first. No model requests are made.

| Build and workload | Binary MiB | Warm median ms | Warm p95 ms | Peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| Default CLI, `--version` | 17.04 | 4.75 | 5.18 | 3.80 |
| `cli,lua`, `--version` | 7.49 | 4.39 | 5.10 | 3.47 |
| No-default-features host example, two tool turns + restore | 0.94 | 3.49 | 4.13 | 2.08 |

The example size is for that linked demonstration, not an estimate of every embedded host.
Freshly built or copied binaries took 234–615 ms on their first observed launches; these
measurements do not isolate code verification, filesystem caching, or machine load. Interactive
readiness, a real provider, long histories, and child processes are outside this baseline.

Proposed acceptance budgets, pending representative hardware runs:

- Warm interactive readiness under 150 ms without remote initialization; startup UI remains
  responsive while optional services connect.
- Idle harness CPU under 0.5% of one core; baseline RSS under 40 MiB without external engines.
- Input-to-paint p95 under 32 ms with a 10,000-line transcript and incoming output.
- Bounded resident transcript and event queue growth during a 100 MiB tool-output stress run;
  completion remains observable and full output remains available on disk.
- Cancellation observed within 250 ms in cooperative waits, with explicit escalation and
  confirmation of child-process cleanup.

These budgets are targets, not enforced or proven guarantees yet. CI exercises independent feature builds, the standalone host, and real-terminal keyboard and
output-lifecycle checks. Broader saturation, crash-recovery, and coding-task benchmarks still
need dedicated CI gates.

## Customization checks

`cargo test --test theme --test header` checks bounded file loading, project precedence, color
validation, and preservation of artwork colors. `python3 tests/tui/themes.py` exercises live
selection, reload, persistence, rejection, and reset in a real pseudo-terminal.
`bun test tests/site` checks designer exports, exact installation contents, overwrite protection,
and local links. Theme files add no scripting engine; the default palette skips the remapping pass.
