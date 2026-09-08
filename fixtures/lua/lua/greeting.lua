return {
  hello = function(name)
    assert(type(name) == "string" and #name > 0, "name is required")
    return "hello, " .. name
  end,
}
