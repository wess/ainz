# Solar CLI comparison

Compared the local Solar CLI checkout at `/Users/wess/Desktop/solar/cli` on October 19, 2026,
and again on November 17, 2026. This compares general agent capabilities. CUI, Lyntris,
HyperGiant gateway/team setup, Jira tickets, enterprise machine policy, FIPS TLS, and
enterprise auditing are outside Ainz's scope.

Ainz keeps its BitchX/irssi interface: timestamped nick lines, hanging wraps, channel topics,
blue status bar, guardian roster, and switchable agent buffers. Alignment means matching
capabilities under that interface.

| Capability | Ainz |
| --- | --- |
| Local, cloud, custom, and headless process providers | Existing; added OpenAI, Anthropic, and OpenRouter presets to setup and CLI |
| Chat Completions | Existing; remains the default for existing HTTP profiles |
| Native Messages | Added streaming, native authentication, images, tool calls, signed/redacted thinking, and usage |
| Native Responses | Added streaming/JSON replies, images, tools, reasoning effort, encrypted reasoning replay, and resumable output items |
| Provider retry and recovery | Existing transient retries; added explicit fallback chains with per-profile models and isolated failed-attempt events |
| Output limits | Added automatic continuation after a reply hits its limit (three in a row without progress pauses), a 32k Chat Completions cap with `max_output_tokens`, and a second request for a reply that broke off partway |
| Context estimate | Added a bytes-per-token ratio learned from the provider's count and a `ctx` status field |
| Headless Claude Code permissions | Added `--permission-prompt-tool stdio`: Claude's requests are decided by Ainz's rules, mode, and approval prompt |
| Provider diagnostics | Existing discovery diagnostics; added `doctor --check-provider` with a 45-second timeout and no retry or fallback |
| Workspace tools, plans, durable jobs | Existing; added shell commands for job start/list/status/output/stop |
| Session trees, rewind, compaction, export | Existing; added named checkpoints and restore |
| Detach, attach, session switching, attention, notifications | Existing |
| Synchronous and background subagents | Existing; added named agent definitions with instructions, tool filters, model, permission, and turn limits; opt-in per-child model choice with `model_hints` |
| Unattended objectives | Added `/goal` with a 25-turn ceiling |
| Images on the prompt | Added `Ctrl+V` clipboard images, several dragged files, and `[Image #N]` placeholders |
| OS sandbox | Added `AINZ_SANDBOX` (`workspace` or `strict`) using `sandbox-exec` or `bwrap`, without Solar's machine-policy floor |
| Instructions, skills, templates, hooks, standing rules | Existing; Ainz retains its own hook and permission configuration format |
| Local/Synapse memory, session search, proposed skills, mesh | Existing |
| MCP stdio and Streamable HTTP | Existing; added remote-server CLI commands and remote launch-profile entries |
| MCP HTTP+SSE and remote sign-in | Added SSE transport, browser OAuth/PKCE, token refresh, login/logout, and environment placeholders |
| Plugins | Existing content-pinned Lua, process, portable packages, and WebAssembly components |
| Settings, keybindings, themes, headers, status fields | Existing; added API, reasoning, and fallback settings |
| Headless operation | Existing ask/JSON/RPC; added positional requests, `-p` with piped input, and `-c` continuation |
| Reported-cost budgets | Added `budget_usd` / `--budget-usd`; stops subsequent requests/tools after reported cost reaches the limit |
| Scrollback, stable reading position, tool output, selection | Existing IRC buffer and terminal-owned transcript selection and copying |
| Embedding | Existing library API without CLI/Lua/WASM defaults; retained |

## Intentional differences

- Ainz loads persistent MCP servers from its own user profile, explicit launch profiles, or
  approved plugins. It does not automatically execute project `.mcp.json` servers. Import
  remains explicit.
- Ainz does not automatically load another harness's executable hooks or permission grants.
  Instructions, skills, prompts, and agent definitions are discovered where they already live.
- Permission modes remain ask, auto, and read-only. Named agent `permissionMode: plan` maps to
  read-only; it does not add a distinct planning UI. Standing edit rules provide unattended edits.
- Model reasoning is retained for protocol continuity and available as JSON events. It does
  not become assistant answer text in the IRC transcript.
- Command and tool names need not match Solar's aliases. Ainz's public Rust API and serialized
  sessions remain its own; older saved messages load without the new optional protocol fields.

## Validation

`tests/recovery.rs`, `tests/limit.rs`, `tests/bridge.rs`, and the sandbox tests (including a
kernel check on macOS that writes outside the workspace and strict-mode connections are
refused) cover the November additions. `tests/parity.rs` checks native requests, tool state after persistence, fallback event isolation,
per-profile fallback models, named-agent discovery/tool restrictions, checkpoint behavior, and
cost budgets. MCP's SSE and OAuth modules include local protocol tests. These do not prove
access to a real cloud model or successful sign-in with a third-party service.
