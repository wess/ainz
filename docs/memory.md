# Memory and self-improvement

A session ends and its transcript is archived. What it worked out on the way — the convention
you corrected, the command that finally worked, the reason a plausible approach is wrong — is
gone with it unless something wrote it down. Ainz has three places to put that: durable
memories, past sessions, and skills.

Everything here is a setting. `/settings` in a session shows the whole list; the CLI equivalents
are below.

## Backends

```toml
[memory]
backend = "local"          # off | local | synapse
recall_on_start = true
recall_limit = 5
remember_on_compact = true
teach = false
```

`local` is the default. Memories are Markdown files under the data directory
(`~/Library/Application Support/ainz/memory` on macOS, `~/.local/share/ainz/memory` on Linux),
one file per memory, with global memories in `global/` and project memories under
`projects/<workspace>-<hash>/`. Nothing leaves the machine and nothing needs installing.

`synapse` stores them in [Synapse](synapse.md) instead, which shares one memory with every
other tool connected to it, and adds supersession, history, and a UI. See that page.

`off` removes the `memory` tool and every behavior below.

```sh
ainz memory backend local
ainz memory add the staging database is named orbit
ainz memory add --global prefers terse commit messages
ainz memory list
ainz memory search staging
ainz memory forget ID
```

## What a session does with it

**Recall at start.** With `recall_on_start`, the newest `recall_limit` memories for the
workspace are put in the system prompt before the first message, marked as context rather than
instruction. A long memory contributes its opening; the session can recall the rest through the
tool.

**The `memory` tool.** `recall` searches, `remember` stores, `forget` removes. The prompt tells
the model not to store what the repository already records.

**Remember before compaction.** With `remember_on_compact`, the moment the transcript is
compacted the session is asked to write down anything durable that is not stored yet. That is
the one point where not having written something down costs you immediately, so it is the one
place Ainz interrupts to ask.

## Searching earlier sessions

Sessions are already durable and already on disk; the `sessions` tool searches them by term and
returns ids with excerpts, and `ainz resume ID` opens one. This is the answer to "we hit this
last week" without having remembered anything in advance.

```sh
ainz sessions --search "certificate error"
```

## Skills a session writes

With `memory.teach` on, a `learn` tool appears. `teach` writes down a procedure the session
worked out, as an Agent Skill; `revise` corrects one that turned out wrong.

The gate is on installing, not on writing. A taught skill lands in `skills/proposed/` under the
config directory, which skill discovery does not read, so writing one costs a line in a list
rather than context in every session. A correction, by contrast, reaches the installed copy
straight away — you already agreed to that skill being loaded, and a correction that never
arrives leaves every session running the version that was wrong. The replaced text is kept
beside it as `SKILL.<timestamp>.md`.

```sh
ainz memory teach on
ainz skills proposed
ainz skills approve cut-a-release
ainz skills reject cut-a-release
```

With the Synapse backend, proposals live in Synapse instead: `synapse skill proposed`,
`synapse skill approve NAME`, `synapse skill history NAME`.
