# Ainz documentation

Start with [installation](install.md), the [web tutorials](https://wess.io/ainz/tutorial/), or the
[terminal interface](interface.md). The [web manual](https://wess.io/ainz/docs/) collects the main
commands and configuration in one place.

The tutorials cover [getting started](https://wess.io/ainz/tutorial/gettingstarted/),
[customizing Ainz](https://wess.io/ainz/tutorial/customizing/),
[writing plugins](https://wess.io/ainz/tutorial/plugins/), and
[advanced operation](https://wess.io/ainz/tutorial/advanced/).

To switch across provider profiles or configure Economy, Balanced, or Quality routing, see
[automatic model routing](providers.md#automatic-model-routing).

| Guide | Covers |
| --- | --- |
| [Install](install.md) | Release binaries, source builds, updates, and uninstalling |
| [Interface](interface.md) | IRC buffer, channel topics, run completion, keys, and steering |
| [Settings](settings.md) | Searchable settings, status-bar fields, and custom shortcuts |
| [Sessions](sessions.md) | Detach, reconnect, multiplex, attention, and notifications |
| [Themes](themes.md) | Theme Designer, palette roles, installation, and live switching |
| [Headers](headers.md) | Mascots, masthead studio, installing and sharing artwork |
| [Providers](providers.md) | Provider profiles, cross-provider model switching, and automatic routing |
| [Tools](tools.md) | Built-in tools and their arguments |
| [Permissions](permissions.md) | Approval modes and standing rules |
| [Hooks](hooks.md) | Start/end hooks, tool hooks, cancellation, and limits |
| [Memory](memory.md) | Recall, session search, and proposed skills |
| [Improvement](improve.md) | Learn from runs, propose harness patches, and verify behavior |
| [Import](import.md) | Reuse existing tool servers and extensions |
| [Lua configuration](lua.md) | init.lua: options, keymaps, commands, events, panels, status bar, tools |
| [Plugins](plugins.md) | Lua, component, and process plugins |
| [MCP](mcp.md) | Stdio, Streamable HTTP, SSE, and remote sign-in |
| [Agents](agents.md) | Named subagent definitions and constraints |
| [Solar comparison](parity.md) | Capability alignment and intentional differences |
| [Synapse](synapse.md) | Shared memory, guidance, skills, and agent mesh |
| [Embedding](embedding.md) | Host the Rust core without the terminal interface |
| [JSON-RPC](rpc.md) | Control a persistent Ainz process |
| [Architecture](architecture.md) | Runtime ownership and component boundaries |
| [Quality](quality.md) | Measured baselines, checks, and remaining reliability work |
| [Release](release.md) | Versioning, packages, and the site publishing path |

The standalone [termweave](../crates/termweave/readme.md) crate documents terminal Markdown
rendering, streaming snapshots, and its ANSI and Ratatui adapters.
