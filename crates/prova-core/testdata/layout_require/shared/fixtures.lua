-- A shared module, reachable only by require() — not a *_test.lua, so discovery ignores it.
-- It defines a fixture and hands back the typed handle: the real cross-suite sharing pattern.
local answer = prova.fixture("answer", Scope.Test, function()
  return 42
end)
return { answer = answer }
