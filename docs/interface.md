# Terminal interface

Ainz uses the dense buffer layout of IRC clients: a channel topic at the top, the conversation
on the left, an agent roster on the right, and activity, status, and input rows at the bottom.
The design takes its cues from irssi and BitchX.

## Read the buffer

```text
#main · Inspect the buffer wrapping
15:31 <you> Check the terminal layout.
15:31 <Ainz> The nick and message share a line. Longer messages
            wrap under the text, keeping the buffer easy to scan.
15:31     ▸ shell  cargo test
15:32   *** Completed · 8s
```

Answers render terminal Markdown: headings, emphasis, lists, links, tables, inline code, and
fenced code. Formatting adapts to the available width. The renderer is
[termweave](../crates/termweave/readme.md), a separate crate with ANSI and Ratatui output.

The title uses a short excerpt of the selected agent's task. While a tool runs, its name and
subject appear there too. Switching agents switches the topic; a finished agent retains its task.
These labels come from existing run data and do not require another model call.

## Know when a run ends

The activity row reports **Working**, **Responding**, **Running tools**, or **Waiting for approval**
with elapsed time. A quiet provider is still active while this row says Working.

**Completed**, **Cancelled**, or **Failed** remains visible above the prompt until the next run.
A matching marker stays in the transcript. Completion waits for end hooks; requesting cancellation
alone is not a completion signal. A run longer than ten seconds can ring the terminal bell.

`Esc` or `Ctrl+C` cancels a running turn, including during tools, compaction, and hooks. A second
`Ctrl+C` abandons a run that has not responded. Finished tool results remain in the conversation;
cancellation does not undo effects a tool already completed.

## Navigate and steer

| Control | Action |
| --- | --- |
| `Ctrl+L` | Toggle the right-hand roster; remembers the choice |
| `Ctrl+1` | Main agent |
| `Ctrl+2` through `Ctrl+9` | Subagent transcripts |
| `Ctrl+=` / `Ctrl+-` | Cycle agents |
| `/agent N` | Select an agent when the terminal cannot send those key chords |
| Wheel, `PageUp` / `PageDown`, `Shift+Up` / `Shift+Down` | Read earlier buffer output |
| `Ctrl+End` | Return to the latest output |
| `Ctrl+O` | Expand or collapse captured tool output, including live output |

Reading earlier output holds your position as new text arrives. Agent key chords require a
terminal supporting the kitty keyboard protocol; the slash command works without it.

Text submitted during a run is steering for the next safe conversation boundary. Up to 32
messages can wait, each at most 64 KiB. A full or closing queue reports the reason and keeps the
draft in the prompt. Cancellation uses a separate signal and does not wait behind steering.

## Edit the prompt

`Up` and `Down` navigate prompt history and return to your draft. `Alt+Left` and `Alt+Right`
move by word; `Ctrl+A` / `Ctrl+E` move to the beginning and end. `Ctrl+U`, `Ctrl+K`, and `Ctrl+W`
cut before the cursor, after it, or the previous word. `Shift+Enter`, or a trailing backslash,
adds a newline. `/vim` enables modal editing.

`/` opens command search; `@` completes workspace paths. Pressing `Esc` twice while idle rewinds
the last prompt and returns it for editing. Hold `Shift` while dragging to use your terminal's
normal text selection. An image pasted or dropped onto the prompt attaches to the next message.

## Customize the session

`/header mascot` selects the mascot at a size suited to the terminal. `/header mascotascii`
selects the plain ASCII version. `/headers` lists available artwork; `/header NAME` previews
and remembers a choice without clearing the conversation. Both commands discover newly added
files, so artwork installed while Ainz is running is available immediately.

Use the [masthead studio](https://wess.io/ainz/masthead/) to draw or extend a header and copy an
installation command. See [headers](headers.md) for files, paths, and limits.

`/settings` controls the roster, header, bell, and editing preferences. `/inline` chooses the
terminal's own scrollback instead of the full-screen buffer on the next launch. Inline mode
has no agent roster or full-screen header preview. Expanded tool output already flushed into
terminal scrollback retains its formatting.

Use the [Theme Designer](https://wess.io/ainz/theme/) to change chat colors. `/themes` lists
installed palettes; `/theme NAME` applies or reloads one immediately, and `/theme default` resets
it. See [themes](themes.md) for the file format and scope. Nick and status formats remain fixed.
