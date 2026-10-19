# Background sessions

Start a session that you can leave running:

```sh
ainz --detach
```

Press `Ctrl+B`, release it, then press `D` to return to your shell. `/detach` does the same.
The agent keeps working. Closing the terminal also leaves a hosted session alive.

Reconnect with `ainz attach`. Choose a session with the arrow keys and Enter. Press `/` to
search by name, workspace, state, or short id; Esc cancels search or returns to the transcript.
The picker groups sessions by workspace and updates when their state changes.

Name a session with `/rename review`, then reconnect with `ainz attach review`. Names must
match one live session; use its id if several sessions share a name. Renaming also saves the
name with the conversation.

## Work across sessions

These shortcuts use a two-key chord: press `Ctrl+B`, release it, then press the second key.
You can change them in **Settings → Keys**.

| Default | Action |
| --- | --- |
| `Ctrl+B D` | Detach to the shell |
| `Ctrl+B S` | Open the session picker; `/sessions` also works |
| `Ctrl+B N` | Start another session in this workspace |
| `Ctrl+B ]` / `Ctrl+B [` | Visit the next / previous live session |
| `Ctrl+B A` | Visit the next session needing attention; `/attention` also works |

Each session keeps its transcript, prompt, provider, and agent runtime. You view one at a time.
Detaching and switching also work while an agent waits for approval. Answer the approval after
reconnecting. Only one terminal can control a session at a time.

Attention navigation visits approvals first, then unseen failures, then unseen completions.
The status bar counts those events in other sessions. A `done · new` or `failed · new` badge
clears when you view that session's transcript. Opening the picker keeps the badge.

```sh
ainz sessions --live          # Live and recently ended hosts, across workspaces
ainz sessions --live --json   # The same information for scripts
ainz attach review           # A unique name, full id, or at least four id characters
ainz stop review             # End the host and its worker
ainz sessions                # Saved conversations in this workspace
ainz resume                  # Resume the latest saved conversation
```

`/exit` ends the session. Use detach to keep it alive. Stopping a host preserves its last saved
conversation; it can lose an unfinished turn. `ainz resume ID` opens saved history and starts a
new runtime. Hosts do not survive a reboot.

## Choose your defaults

Open `/settings`, `/config`, or F2. Under **Sessions**, turn on **Background sessions** to host
future interactive launches. The current foreground session stays in the foreground. Use
`ainz --no-detach` to override the preference for one launch or for piped input.

Hosted sessions use the full-screen interface. Foreground sessions keep your inline preference.
Each hosted session runs its own host and worker; idle sessions wait for events. There is no
shared daemon to keep running after sessions end. Live hosting requires macOS or Linux.

## Notifications

Notifications start off. In **Settings → Sessions**, choose **Needs input** for approval
alerts, or **Needs input + finished** for approvals, completions, and failures.

**Terminal** delivery uses OSC 9 in an attached terminal that supports it. **System** delivery
uses `osascript` on macOS or `notify-send` on Linux, including while detached; the operating
system must allow notifications and the command must be available. Alerts for the transcript
you are viewing stay quiet. Reconnecting does not replay old alerts.

```toml
[ui]
background = true
notifications = "needs_input" # off, needs_input, all
notification_delivery = "terminal" # terminal, system
```

## Named conversation checkpoints

```sh
ainz checkpoint                         # latest saved session, label "manual"
ainz checkpoint SESSION_ID before
ainz restore SESSION_ID before
```

A checkpoint is a separate private snapshot of the conversation. Taking it does not change
the active path. Restoring replaces the saved session; stop its live host first. Checkpoints
remain available after restore. These are conversation snapshots, not filesystem backups.
