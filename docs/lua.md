# Lua configuration

Ainz runs your own Lua the way Neovim does. An `init.lua` sets options, maps keys, adds slash
commands, answers events, opens columns beside the transcript, rewrites the status bar and
topic line, and gives the model new tools. Plugins are more Lua files beside it. Everything is
exposed through one global, `ainz`, the way Neovim exposes `vim`.

Scripts run with your full authority: the whole standard library, including `io` and `os`.
They are your configuration, like a shell profile. That is different from
[plugins](plugins.md) with a `plugin.toml`, which are sandboxed, capability-limited, and
approved one at a time.

Without an `init.lua` no Lua runtime is created, so a session that doesn't use any of this
pays nothing for it.

## Where it lives

```text
<config dir>/init.lua              your configuration (runs first)
<config dir>/lua/NAME.lua          require("NAME")
<config dir>/plugin/*.lua          run after init.lua, in name order
<config dir>/pack/*/start/*/       packages: each has its own plugin/ and lua/
<workspace>/.ainz/init.lua         a project's own script, run once trusted
```

The config directory is the one `config.toml` lives in: `~/Library/Application Support/ainz` on
macOS, `~/.config/ainz` on Linux, or wherever `AINZ_CONFIG` points. `~/.config/ainz/init.lua` is
also found on macOS, for anyone who keeps everything there. `AINZ_INIT=PATH` names a different
file, `AINZ_INIT=NONE` runs no scripts, and so does `ainz --clean`.

A package is a folder you can `git clone` straight into `pack/plugins/start/`, the way Neovim
packages work: its `plugin/*.lua` runs at startup and its `lua/` folder is on the `require` path.

A project's `.ainz/init.lua` arrives with a checkout rather than from you, so it does not run
until you trust it. `/trust` records it as it is now and runs it. Editing it makes it untrusted
again until you `/trust` it once more.

Scripts run for interactive sessions and for `ainz -p` and `ainz ask`, not for configuration
commands such as `ainz providers`. In a headless run, what a script prints goes to stderr.

## Options

`ainz.opt` (or `ainz.o`) is the whole configuration, with the same names as `config.toml`:

```lua
ainz.opt.model = "sonnet"
ainz.opt.permissions = "ask"           -- ask | auto | read_only
ainz.opt.max_steps = 64
ainz.opt.ui.vim = true
ainz.opt.ui.theme = "midnight"
ainz.opt.providers.litellm.max_output_tokens = 64000
ainz.opt.subagent_models = true
```

Options a script sets belong to the session, like `:set` in Neovim: they are never written to
`config.toml`. A change you make in `/settings` is still saved, and only that change. Command-line
flags such as `--model` apply after `init.lua`, so they have the last word.

A misspelt name or a value of the wrong type is reported and skipped; the rest still apply. An
option set from a keymap, command, or timer takes effect at once. Reading `ainz.opt` always
gives the configuration as it stands, including what `/settings` just changed.

`ainz.g` is a table for your own settings, shared by all your scripts.

## Keymaps

```lua
ainz.keymap.set("ctrl+g", function() ainz.notify("hello") end, { desc = "say hello" })
ainz.keymap.set("ctrl+b g", "/goal continue")     -- a chord; a string is sent as if typed
ainz.keymap.set("f5", "roster")                    -- a built-in action moves to this key
ainz.keymap.del("ctrl+g")
```

Keys are written the way [settings](settings.md#choose-your-shortcuts) writes them: `ctrl+`,
`alt+`, `shift+`, `cmd+` (or `super+`), letters, `f1`–`f12`, and navigation keys. A mapping must
start with a modifier or a function key, so a script never takes a letter away from typing. Your
single-key mappings answer before the built-in shortcuts, so one can replace a default. A chord
can extend the built-in `ctrl+b` prefix.

Built-in action names are `detach`, `sessions`, `new_session`, `next_session`,
`previous_session`, `next_attention`, `settings`, `interrupt`, `quit`, `send`, `newline`, `clear`,
`roster`, `mesh_roster`, `detail`, `next_agent`, `previous_agent`, `scroll_up`, `scroll_down`, and
`permissions`.

## Commands

```lua
ainz.command("standup", function(e)
  ainz.send("Summarise what changed in this repository since " .. (e.args ~= "" and e.args or "yesterday"))
end, { desc = "draft a standup note" })
```

`/standup monday` calls the function with `e.args` (`"monday"`), `e.fargs` (the words), and
`e.name`. Your commands appear in the `/` palette and in `/help`.

## Events

Like irssi signals or Neovim autocommands:

```lua
local id = ainz.on("turn_end", function(e)
  if not e.ok then ainz.notify("that run failed: " .. (e.error or "cancelled"), "warn") end
end)
ainz.off(id)
```

| Event | Carries |
| --- | --- |
| `start`, `exit` | nothing; the interface opened or is closing |
| `prompt` | `text`. Return a string to replace it, or `false` to keep it from being sent |
| `pre_tool` | `id`, `name`, `arguments`, `subject`. Return `false, "reason"` to refuse the call |
| `tool_start` | `id`, `name`, `arguments`, `subject` |
| `tool_end` | `id`, `error`, `output` (first 4 KiB) |
| `turn_end` | `ok`, `cancelled`, and `text` or `error` |
| `model_selected` | Auto worker `target` with `provider` and `model`, plus `strategy` and `reason` |
| `notice`, `error` | `message` |
| `context` | `used`, `limit`: the share of the context window the last request took |
| `compaction` | `archived_messages`, `summary` |
| `todos` | `items` |
| `steering` | `message` |
| `cancelled` | nothing |
| `subagent_start` | the child's `session_id`, `parent_id`, `name`, `task`, and model `target` |
| `subagent_end` | the child's `session_id` and `error` |
| `worker_update` | a process provider's worker `id`, `name`, `task`, `status`, and `detail` |
| `resize` | `width`, `height` |

`pre_tool` runs before every tool call, in every permission mode, subagents included, and in
headless runs too. It is a gate like a `pre_tool` [hook](hooks.md) without starting a process. A
handler that errors refuses the call: a broken check is not a passed one.

```lua
ainz.on("pre_tool", function(call)
  if call.name == "shell" and call.subject:find("git push", 1, true) then
    return false, "pushing is mine to do"
  end
end)
```

Streaming text and live tool output are not events. They arrive a piece at a time, and a
handler on each would cost more than it could show.

## Timers

```lua
local clock = ainz.timer(1000, function() ainz.statusbar.refresh() end, { ["repeat"] = true })
clock:stop()
ainz.defer(500, function() ainz.notify("half a second later") end)
```

A repeating timer runs at most ten times a second. With no timers, an idle session sleeps.

## Panels

A panel is a column beside the transcript, on the left or the right, for anything you want to
keep in view:

```lua
local notes = ainz.panel.open({ side = "left", width = 24, title = "notes" })
notes:set({ "%Gtodo%n", "  ship 0.17", { "  blocked", fg = "error" } })

local log = ainz.panel.open({ side = "right", width = 30, title = "tools", follow = true })
ainz.on("tool_start", function(call)
  log:append("%c" .. call.name .. "%n " .. (call.subject or ""))
end)
```

`side` is `"left"` or `"right"` (default). `width` is 8–80 columns (default 28). `height` fixes
the rows; without it, panels on a side share the column. `follow = true` shows the newest lines,
like a log window. The roster shares the right column, below any right-hand panels.

`panel:set(lines)` replaces the content, `panel:append(line)` adds a line and keeps the last
thousand, `panel:clear()`, `panel:title(text)`, and `panel:close()`. Panels are left out when
the terminal is too narrow to keep 40 columns for the transcript.

## Status bar and topic

The bar above the prompt works like an irssi statusbar: built-in items and your own, in the
order you give.

```lua
ainz.statusbar.item("clock", function() return "%Y" .. os.date("%H:%M") .. "%n" end)
ainz.statusbar.item("cost", function(s) return s.cost and string.format("$%.2f", s.cost) or "" end)
ainz.statusbar.set({ "clock", "model", "mode", "context", "cost", "agents" })
ainz.timer(30000, ainz.statusbar.refresh, { ["repeat"] = true })
```

Built-in items are the [status fields](settings.md#customize-the-status-bar): `provider`, `model`,
`mode`, `memory`, `tokens`, `inputtokens`, `outputtokens`, `context`, `todos`, `agents`,
`workspace`, `session`, `activity`, `attention`, and `branch`. An item that returns an empty string
is left out. `ainz.statusbar.set(nil)` goes back to the default.

To draw the whole bar yourself:

```lua
ainz.statusbar.render(function(s)
  return string.format(" %%_%s%%_ │ %s │ ctx %d%%%% ", s.model, s.mode, s.context and s.context.percent or 0)
end)
```

`ainz.topic.render(fn)` draws the topic line at the top the same way. Pass `nil` to either to
return to the default.

Item, bar, and topic functions get the same state `ainz.state()` returns:

```lua
{ provider, model, mode, busy, activity, input, workspace, branch, session, cost,
  tokens = { input, output, total }, context = { used, limit, percent },
  agents = { running, total }, todos = { done, total }, width, height }
```

They run when the screen is drawn, so keep them quick. A script that runs longer than two
seconds is stopped.

`provider` and `model` describe the selected buffer. In an Auto session they show the chosen
worker once routing completes.

## Format codes

Text in panels and bars uses irssi and BitchX colour codes:

| Code | Meaning |
| --- | --- |
| `%k %r %g %y %b %m %c %w` | black, red, green, yellow, blue, magenta, cyan, white |
| `%K %R %G %Y %B %M %C %W` | the bright versions |
| `%0`–`%7` | background colour, in the same order |
| `%_` `%U` `%I` `%8` | bold, underline, italic, reverse (each toggles) |
| `%n` | back to plain |
| `%{accent}` | a [theme](themes.md) colour, which follows `/theme` |
| `%{#ff8800}` `%{bg:#202020}` | any colour, foreground or background |
| `%%` | a literal `%` |

A line can also be a table: `{ "text", fg = "red", bg = "#202020", bold = true }`, or a list of
strings and such tables.

`ainz.highlight("accent", "#ff8800")` changes a theme colour for the session. The roles are
`background`, `text`, `muted`, `accent`, `success`, `bar`, `info`, `warning`, `error`,
`special`, `bright`, `bar_text`, and `border`.

## Tools for the model

```lua
ainz.tool({
  name = "weather",
  description = "Current weather for a city",
  risk = "network",                  -- read | write | execute | network
  parameters = {
    type = "object",
    properties = { city = { type = "string" } },
    required = { "city" },
  },
  run = function(args, call)
    local handle = io.popen("curl -s 'wttr.in/" .. args.city .. "?format=3'")
    local out = handle:read("*a")
    handle:close()
    return out
  end,
})
```

`risk` decides whether a call asks first, the same as for built-in tools, and standing rules
apply by name. `run` gets the arguments and `call.workspace`, and returns a string, or a table
that is sent as JSON. A tool runs off the interface's thread and may take up to two minutes.

## Doing things

| Function | Does |
| --- | --- |
| `ainz.notify(text, level)` | A line in the buffer; `level` is `"info"`, `"warn"`, or `"error"` |
| `print(...)` | The same, at info |
| `ainz.send(text)` | Sends text as if typed: a prompt, steering during a run, or a slash command |
| `ainz.cmd("model")` | Runs `/model` |
| `ainz.input.get()`, `.set(text)`, `.insert(text)` | The prompt line |
| `ainz.state()` | The state table above |
| `ainz.redraw()` | Draws again |
| `ainz.quit()` | Leaves once the current run ends |
| `ainz.inspect(value)` | A readable string for any value |
| `ainz.json.encode(value)`, `ainz.json.decode(text)` | JSON |
| `ainz.version`, `ainz.workspace` | Strings |

`ainz.send` waits until the prompt is free: never over a permission prompt, and never into a
subagent's transcript. What a script sends stays out of your prompt history, and your draft
goes back in the line afterwards.

## Reloading and trying things

`/source` runs your scripts again: everything they registered is dropped first, then they run.
Options they set are applied again. One they no longer set keeps its value until Ainz starts
again, as in Neovim.

`/lua CODE` runs Lua in the same runtime and shows the value of an expression:
`/lua ainz.opt.model`, `/lua #ainz.state().input`. `/lua` alone lists what was loaded.

A script that fails at load is reported in the buffer and the rest still run. One that fails in a
callback is reported and the session carries on.

## An example

[`examples/init.lua`](../examples/init.lua) is a BitchX-flavoured setup: a clock and cost on the
bar, a channel-style topic, a tool log down the right, notes on the left, a few keymaps and
commands, and a gate on `git push`.
