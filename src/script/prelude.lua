-- The part of the `ainz` API that is easier to say in Lua than in Rust: options that know when
-- they are written, objects for panels and timers, `print`, and `inspect`. Loaded into every
-- runtime before init.lua, and again on /source.

local ainz = ...

-- ainz.opt reads the configuration as it stands and records each write, so a write made from a
-- keymap or a timer reaches the running session as well as one made at startup.
local function walk(path)
  local node = ainz.__opt
  for i = 1, #path do
    if type(node) ~= "table" then
      return nil
    end
    node = node[path[i]]
  end
  return node
end

-- a write at any depth, making the tables on the way when the configuration has none there yet
local function assign(path, value)
  local node = ainz.__opt
  for i = 1, #path - 1 do
    if type(node[path[i]]) ~= "table" then
      node[path[i]] = {}
    end
    node = node[path[i]]
  end
  node[path[#path]] = value
  ainz.__write(path, value)
end

local function proxy(path)
  return setmetatable({}, {
    __index = function(_, key)
      local child = { table.unpack(path) }
      child[#child + 1] = key
      local value = walk(child)
      if type(value) == "table" then
        return proxy(child)
      end
      return value
    end,
    __newindex = function(_, key, value)
      local child = { table.unpack(path) }
      child[#child + 1] = key
      assign(child, value)
    end,
    __pairs = function()
      return next, walk(path) or {}, nil
    end,
    __len = function()
      local node = walk(path)
      return type(node) == "table" and #node or 0
    end,
  })
end

ainz.opt = proxy({})
ainz.o = ainz.opt
ainz.g = ainz.g or {}

-- a keymap whose right-hand side names a built-in action moves that action's shortcut
local set = ainz.keymap.__set
ainz.keymap.set = function(keys, rhs, opts)
  if type(rhs) == "string" and ainz.__actions[rhs] then
    assign({ "ui", "keybindings", rhs }, keys)
    return
  end
  return set(keys, rhs, opts or {})
end

local Panel = {}
Panel.__index = Panel
function Panel:set(lines)
  ainz.__panel_set(self.id, lines)
  return self
end
function Panel:append(line)
  ainz.__panel_append(self.id, line)
  return self
end
function Panel:clear()
  return self:set({})
end
function Panel:title(text)
  ainz.__panel_title(self.id, text)
  return self
end
function Panel:close()
  ainz.__panel_close(self.id)
end

ainz.panel = {
  open = function(opts)
    return setmetatable({ id = ainz.__panel_open(opts or {}) }, Panel)
  end,
}

local Timer = {}
Timer.__index = Timer
function Timer:stop()
  ainz.__timer_stop(self.id)
end

ainz.timer = function(ms, fn, opts)
  opts = opts or {}
  return setmetatable({ id = ainz.__timer(ms, fn, opts["repeat"] == true) }, Timer)
end
ainz.defer = function(ms, fn)
  return ainz.timer(ms, fn)
end

local function inspect(value, depth, seen)
  depth = depth or 0
  seen = seen or {}
  if type(value) == "string" then
    return string.format("%q", value)
  end
  if type(value) ~= "table" then
    return tostring(value)
  end
  if seen[value] then
    return "<cycle>"
  end
  if depth > 4 then
    return "{...}"
  end
  seen[value] = true
  local keys = {}
  for key in pairs(value) do
    keys[#keys + 1] = key
  end
  table.sort(keys, function(a, b)
    return tostring(a) < tostring(b)
  end)
  local parts = {}
  local pad = string.rep("  ", depth + 1)
  for _, key in ipairs(keys) do
    local name = type(key) == "string" and key:match("^[%a_][%w_]*$") and key
      or "[" .. inspect(key, depth + 1, seen) .. "]"
    parts[#parts + 1] = pad .. name .. " = " .. inspect(value[key], depth + 1, seen)
  end
  seen[value] = nil
  if #parts == 0 then
    return "{}"
  end
  return "{\n" .. table.concat(parts, ",\n") .. "\n" .. string.rep("  ", depth) .. "}"
end
ainz.inspect = inspect

-- print goes to the buffer, the way it goes to the message area in Neovim
print = function(...)
  local parts = {}
  for i = 1, select("#", ...) do
    local value = select(i, ...)
    parts[i] = type(value) == "table" and inspect(value) or tostring(value)
  end
  ainz.notify(table.concat(parts, "\t"))
end
