local greeting = require("greeting")

return {
  hello = function(args)
    return { message = greeting.hello(args.name) }
  end,
}
