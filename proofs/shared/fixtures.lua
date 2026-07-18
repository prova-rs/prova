-- Shared across proofs, reached as `require("shared.fixtures")` because the require-root is `proofs/`.
-- Returns typed handles; each suite that requires it builds its own instance (shared recipe,
-- isolated instance).
local greeting = prova.fixture("greeting", Scope.Test, function()
  return "hello from shared/"
end)
return { greeting = greeting }
