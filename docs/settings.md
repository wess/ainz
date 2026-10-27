# Settings and shortcuts

Open `/settings`, `/config`, or press F2. Both commands open the same screen. Select
**Provider and model** to open connection setup; `/model` opens that setup directly.

Press Tab or Shift+Tab to change sections: **All**, **Connection**, **Interface**, **Sessions**,
**Keys**, and **Memory**. Press `/` to search labels and section names. Enter leaves search
and lets you change the selected result. Esc clears a search, then closes settings.

Use Up/Down to select a row and Enter or Left/Right to change its value. Read the details
beside the row before changing it. Close settings to save. Runtime setup changes apply when
chat reopens; background hosting applies at the next launch.

**Memory → Automatic improvement** enables background experiments after saved runs. Turn it
on once, then use Ainz normally; result notices appear in the conversation. `/improve` reviews
the latest patch and proposed skills, and `/improve approve` accepts the reviewed result. See
[improving the harness](improve.md) for limits, drift guards, and source handling.

## Customize the status bar

`/statusline` or `/statusbar` opens a field picker for the IRC bar above the prompt. You can
also open **Interface → Status bar fields** in settings.

- Space toggles a field.
- Shift+Up/Down moves the selected field.
- Enter saves the selection and order.
- R restores the responsive default; Esc discards the edit.

Turn off all fields to hide the bar. For a quick change:

```text
/statusline model branch tokens attention
/statusline off
/statusline reset
```

Available fields: `provider`, `model`, `mode`, `memory`, `tokens`, `inputtokens`,
`outputtokens`, `context`, `todos`, `agents`, `workspace`, `session`, `activity`, `attention`,
and `branch`. Context shows the configured token limit. Branch reads local repository metadata,
including worktrees, and stays empty outside a repository. Ainz does not run a Git command to
read it. Attention counts approvals and unseen results in other live sessions.

With no override, Ainz keeps its responsive status layout. Narrow terminals clip custom fields
from the right, so put the fields you need most first.

```toml
[ui]
status_line = ["model", "branch", "tokens", "attention"]
```

An empty list hides the bar. Remove `status_line` to restore the default.

## Choose your shortcuts

Select an action under **Keys** and press Enter. Press one combination or the two keys of a
chord, then Enter to save. Press Backspace before recording to restore its default, Delete to
disable an optional action, or Esc to cancel. Ainz explains conflicts before accepting a change.
Send, cancel, and quit must keep shortcuts. Ordinary typing and Esc remain available.

You can also edit `[ui.keybindings]` in the config file:

```toml
[ui.keybindings]
detach = "alt+d"
sessions = "ctrl+x s"
settings = "f6"
```

Use `ctrl`, `alt`, `shift`, or `super` with a key. `cmd` and `option` are accepted aliases.
Two-key chords use a space, such as `ctrl+b d`. Use `none` to disable an optional action.
Function keys F1–F12 and navigation names such as `pageup`, `enter`, and `tab` also work.
Ainz validates the complete set, including default bindings, when it loads the config.

Actions: `detach`, `sessions`, `new_session`, `next_session`, `previous_session`,
`next_attention`, `settings`, `interrupt`, `quit`, `send`, `newline`, `clear`, `roster`, `mesh_roster`,
`detail`, `next_agent`, `previous_agent`, `scroll_up`, `scroll_down`, and `permissions`.
`/help` lists your current bindings. A terminal can intercept shortcuts before Ainz receives
them; choose another combination if that happens.

The defaults keep Ainz's editing controls. Shift+Enter inserts a newline, Ctrl+U cuts before
the cursor, Ctrl+C cancels a run or exits an idle session, and Ctrl+D exits an empty idle
session. Shift+Tab cycles Ask, Auto, and Read-only; the new mode applies to the next run.
