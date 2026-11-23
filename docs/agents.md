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

`model` optionally chooses another model on the parent's provider. Set `provider` with `model`
to pin a pair on a different profile. `inherit` keeps the parent's selection. `tools` and
`disallowedTools` are comma-separated names. Familiar aliases
`Bash`, `Grep`, `LS`, and `WebFetch` map to `shell`, `search`, `list`, and `fetch`.
A child's permission mode cannot widen its parent's mode, and its turn limit cannot exceed
the parent's. Standing rules still apply. `plan` maps to read-only.

## Choosing a model for each child

With `subagent_models = true` in `config.toml`, or **Subagent models** turned on in
`/settings`, the subagent tool offers saved models across configured profiles. The `model`
parameter selects a model, and `provider` selects its profile when it differs from the parent.
The agent can then send a bounded lookup to a small
model and a design question to a strong one. It needs two or more saved models. Add a few
words for each under `model_hints` so the agent knows what each model is for:

```toml
subagent_models = true

[providers.litellm]
models = ["haiku", "sonnet", "opus"]

[providers.litellm.model_hints]
haiku = "fast and cheap; lookups, summaries, mechanical edits"
opus = "strongest reasoning; design reviews and hard debugging"
```

The tool validates the selected pair against the saved list. A named type's provider and model
take precedence. Leave them out when delegating to a named type, or supply the same values.
A child with no model selection inherits the parent's worker, including in Auto.

The child gets the definition's instructions in addition to workspace instructions. It keeps
its own durable session, events, and Synapse seat when mesh is enabled. Definitions alone do
not start a child.
