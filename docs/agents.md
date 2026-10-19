# Named agents

Subagents keep their guardian names and their own switchable IRC buffers. A named agent type
adds instructions and can narrow a child's tools, permissions, model, and number of turns.

Definitions are Markdown files in `~/.claude/agents`, the Ainz config directory's `agents/`,
and `.claude/agents` or `.ainz/agents` from the filesystem root down to the workspace.
The nearest definition wins by name. `ainz agents` lists the discovered types. The model sees
the catalog and selects a type through the subagent tool's `role` argument.

```markdown
---
name: reviewer
description: Review changes without modifying the workspace
tools: read, list, search
disallowedTools: shell
permissionMode: read-only
maxTurns: 8
---
Read the implementation and tests. Report concrete bugs with file references.
```

`model` optionally chooses another model on the configured provider; `inherit` keeps the
parent's model. `tools` and `disallowedTools` are comma-separated names. Familiar aliases
`Bash`, `Grep`, `LS`, and `WebFetch` map to `shell`, `search`, `list`, and `fetch`.
A child's permission mode cannot widen its parent's mode, and its turn limit cannot exceed
the parent's. Standing rules still apply. `plan` maps to read-only.

The child gets the definition's instructions in addition to workspace instructions. It keeps
its own durable session, events, and Synapse seat when mesh is enabled. Definitions alone do
not start a child.
