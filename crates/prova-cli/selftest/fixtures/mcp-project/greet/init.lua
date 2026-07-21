-- A minimal local plugin with a LuaCATS stub, so the self-tests can prove that a declared
-- plugin's API surfaces in introspection (autodidact M4).
local greet = {}

--- Compose a greeting.
function greet.hello(name)
  return "hello, " .. (name or "world")
end

return greet
