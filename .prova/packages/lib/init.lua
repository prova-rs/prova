-- A library package that privately depends on `inner`. It composes inner's value into its OWN
-- surface (`derived`) — proving a library CAN use a dependency internally — while never re-exporting
-- `inner`. A consumer that requires `lib` must get `lib`, and NOT `inner`.
--
-- These `require`s bind to lib's own dependency map (`prova.toml [dependencies]`) through a
-- package-scoped `require`, so `inner` resolves here and nowhere else. Nothing about these lines
-- looks special — that is the point: a package author writes an ordinary require and gets privacy by
-- declaring the dependency, not by using a different API.
local inner = require("inner")

local M = {}

M.derived = inner.stamp(inner.secret) -- "inner-secret::stamped-by-inner"

-- Deliberately LAZY: the require runs inside a function, at call time — long after this chunk
-- finished loading. Scoping happens at load by binding the chunk's environment, which is exactly why
-- a test-time require still resolves against lib's private map and not the caller's namespace.
function M.lazy_secret()
  return require("inner").secret
end

return M
