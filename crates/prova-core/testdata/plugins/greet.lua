-- A user-authored plugin resolved from a declared disk root. Returns a namespace table,
-- exactly like a first-party module — proving third-party plugins have no privileged difference.

local greet = {}

function greet.hello(name)
  return "hello, " .. (name or "world")
end

return greet
