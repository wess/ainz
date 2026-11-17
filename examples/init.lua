-- A BitchX-flavoured Ainz. Copy it to the config directory as init.lua (the folder config.toml
-- lives in), then /source. See docs/lua.md for everything used here.

ainz.opt.ui.vim = false
ainz.opt.ui.bell = true

-- the bar: a clock, the model and mode, how full the context is, and what the session cost
ainz.statusbar.item("clock", function()
  return "%Y" .. os.date("%H:%M") .. "%n"
end)
ainz.statusbar.item("ctx", function(s)
  if not s.context then
    return ""
  end
  local colour = s.context.percent > 75 and "%R" or s.context.percent > 50 and "%Y" or "%G"
  return "ctx " .. colour .. s.context.percent .. "%%%n"
end)
ainz.statusbar.item("cost", function(s)
  return s.cost and string.format("$%.2f", s.cost) or ""
end)
ainz.statusbar.set({ "clock", "model", "mode", "ctx", "cost", "agents", "branch" })
ainz.timer(30000, ainz.statusbar.refresh, { ["repeat"] = true })

-- the topic line, the way a channel shows one
ainz.topic.render(function(s)
  local what = s.busy and "%Cworking%n" or "%Gready%n"
  local where = s.workspace:match("[^/]+$") or s.workspace
  return "%_#" .. where .. "%_ %K│%n " .. what .. " %K│%n " .. s.provider .. "/" .. s.model
end)

-- a log of tool calls down the right, newest at the bottom
local log = ainz.panel.open({ side = "right", width = 30, title = "tools", follow = true })
ainz.on("tool_start", function(call)
  log:append(os.date("%H:%M ") .. "%c" .. call.name .. "%n " .. (call.subject or ""))
end)
ainz.on("tool_end", function(result)
  if result.error then
    log:append("      %Rfailed%n")
  end
end)

-- notes on the left, kept in a file in the workspace, toggled with ctrl+n
local notes
local function show_notes()
  local file = io.open(ainz.workspace .. "/NOTES.md")
  local lines = {}
  if file then
    for line in file:lines() do
      lines[#lines + 1] = line
    end
    file:close()
  else
    lines[1] = "%Kno NOTES.md here%n"
  end
  notes = ainz.panel.open({ side = "left", width = 26, title = "notes" })
  notes:set(lines)
end
ainz.keymap.set("ctrl+n", function()
  if notes then
    notes:close()
    notes = nil
  else
    show_notes()
  end
end, { desc = "toggle the notes column" })

-- commands
ainz.command("standup", function(e)
  ainz.send("Summarise the commits in this repository since " .. (e.args ~= "" and e.args or "yesterday") .. " as a short standup note.")
end, { desc = "draft a standup note" })
ainz.command("cost", function()
  local s = ainz.state()
  ainz.notify(string.format("%d tokens in, %d out%s", s.tokens.input, s.tokens.output,
    s.cost and string.format(", $%.2f", s.cost) or ""))
end, { desc = "what this session has used" })

-- a gate: the model may commit, but pushing is mine
ainz.on("pre_tool", function(call)
  if call.name == "shell" and (call.subject or ""):find("git push", 1, true) then
    return false, "pushing is left to the person"
  end
end)

-- when a long run finishes in another window, say so on the buffer too
ainz.on("turn_end", function(e)
  if not e.ok and not e.cancelled then
    ainz.notify("run failed: " .. (e.error or "?"), "warn")
  end
end)
