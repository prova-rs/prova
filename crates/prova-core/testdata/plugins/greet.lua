-- A user-authored plugin resolved from disk via PROVA_PLUGIN_PATH. Returns a namespace table,
-- exactly like a first-party module — proving third-party plugins have no privileged difference.

local greet = {}

function greet.hello(name)
  return "hello, " .. (name or "world")
end

return greet
