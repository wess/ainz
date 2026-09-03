# Permissions

Every tool call carries a risk — read, write, execute, or network; see
[`docs/tools.md`](tools.md) for what each built-in tool is. What happens next is decided, in
order: first any standing rule that already covers this call, then the permission mode, then a
`pre_tool` hook that can still refuse it. A call the rules or the mode already denied never
reaches a hook. See [`docs/hooks.md`](hooks.md) for what a hook sees and can do.

## Modes

```toml
permissions = "ask"   # ask | auto | read_only
```

- `ask` (the default) — a read runs without asking; a write, execute, or network call prompts
  for approval.
- `auto` — everything runs without asking.
- `read_only` — a read runs; everything else is denied outright, with nobody asked.

Set it with `--permissions ask|auto|read-only` on the command line, `/permissions MODE` (or
`/settings`) in a session, or the `permissions` key in `config.toml`. A surface with nobody to
ask — `ainz ask --json`, `ainz rpc` — never opens a prompt, so under `ask` a write or execute call
is denied there exactly as it would be if a human answered no; pass `--permissions auto` to let
it proceed, or cover the specific call with a standing rule instead.

`--yeet`, and `/yeet` in a session, is not a fourth mode: it sets `auto` and, on top of that,
loads every discovered plugin as though it were approved, for this run only. Nothing is written
to disk, so both effects end with the process. Choosing a mode with `/permissions` or in
`/settings` ends yeet early. See [`docs/plugins.md`](plugins.md) for the plugin-trust half of
what it does.

## Standing rules

`[rules]` in `config.toml` holds patterns that are decided before the mode is even consulted, in
every mode, on every surface — including a headless one that has nobody to ask. A match overrides
the mode in both directions: an `allow` rule lets a write through under `read_only`, and a `deny`
rule stops one even under `auto`.

```toml
[rules]
allow = ["read", "shell(git *)"]
deny = ["write(notes.md)"]
```

A pattern is a tool name on its own (`read`), or a tool name with a prefix of the call's subject
in parentheses (`shell(git *)`, `write(notes.md)`). The subject is whichever of `command`,
`path`, `file_path`, `pattern`, `url`, or `query` the call's arguments carry. A prefix ending in
`*` matches anything that starts with it; without one, the subject has to match exactly. `deny`
is checked first and wins, so taking an allowance back is one line in `deny` rather than an edit
to `allow`. A call that matches neither list falls through to the mode, unchanged.

At a permission prompt, `y` allows the call once and `n` refuses it; `a` allows it and also
writes a rule for it to `[rules] allow`, so the same decision is not asked again. For a `shell`
call the rule remembers only the command's first word (`shell(git *)`, not the exact command
line the model happened to run); every other tool remembers its bare name.

`/rules` (or `/permissions rules`) lists the standing allow and deny rules. `/rules clear` (or
`/permissions rules clear`) forgets all of them, so every call asks again. There is no CLI or
in-session way to add a `deny` rule — write one into `config.toml` directly.
