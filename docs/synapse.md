# Synapse

[Synapse](https://wess.io/synapse/) keeps memory, one skill library, and an agent mesh on your
machine, shared by the tools you already use. Ainz can use all three. It is optional in both
directions: the setting has to be on and the binary has to be installed, and Ainz runs the same
without it.

## Turning it on

```sh
ainz synapse            # what is on, and where the binary is
ainz synapse enable
ainz memory backend synapse
ainz synapse mesh on
ainz synapse disable
```

`/settings` does the same in a session. When Synapse is installed and this is your first run,
Ainz offers it once after provider setup and takes no for an answer.

```toml
[synapse]
enabled = false
mesh = false
# command = "/absolute/path/to/synapse"   # only when it is not on PATH
```

Setting `memory.backend = "synapse"` implies `enabled`, so the two cannot disagree.

## What it adds

**A tool server.** Ainz registers `synapse mcp` for the session, so every Synapse tool is
reachable through the `mcp` tool without appearing in the profile or in the prompt.

**Guidance.** Synapse's own instructions, including your `SOUL.md`, are appended to the system
prompt — the same guidance your other connected tools load.

**Memory.** With `memory.backend = "synapse"`, recall and remember go to Synapse rather than to
local files, so a decision recorded here reaches a later session in Claude Code or Codex, and
one recorded there arrives in Ainz. See [memory](memory.md).

**Skills.** With `memory.teach` on, `teach` and `revise` write into the Synapse skill library,
which installs approved skills into every connected tool rather than into one.

**The mesh.** With `synapse.mesh` on, the session registers under the workspace name and each
subagent registers under its own guardian name. They become addressable: `send` messages one,
`waitstatus` blocks until one reaches a state, `reportstatus` tells the person watching what is
happening, and you have a seat of your own in Synapse's console, so an agent that hits a
decision it should not make alone has somebody to ask.

A mesh subagent gets its own client rather than sharing its parent's, so it holds a seat of its
own; that also means it starts its own copy of any tool server it uses.

Registration is best effort everywhere. A Synapse that will not start, or a mesh that is off in
Synapse's own settings, costs the session that feature and never its startup.

## Without the setting

Registering the server by hand still works, and gives you the tools without the memory, guidance,
or mesh integration:

```sh
ainz mcp add synapse --required -- /path/to/synapse mcp
```

Synapse can also pass `--mcp-config FILE` when it launches Ainz. Harness integration goes through
[JSON-RPC mode](rpc.md).
