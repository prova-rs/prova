-- The out-of-the-box `shared` package: require("shared"). It registers fixtures (side effect) and
-- returns typed handles + helpers. Each suite that requires it builds its OWN instances (shared
-- recipe, isolated per suite). Delete this package if you don't want it; lift it to a git repo and
-- declare it in [dependencies] to share across projects — the `require("shared")` never changes.
local M = {}

-- A shared fixture, used by handle: `local S = require("shared"); t:use(S.greeting)`.
M.greeting = prova.fixture("greeting", Scope.Test, function()
  return "hello from the shared package"
end)

-- A plain helper — packages are just libraries.
function M.slugify(s)
  return (s:lower():gsub("%s+", "-"))
end

-- Where this package's own file lives, from the per-package `pkg` table. A package uses this to
-- find ITS OWN repo's artifacts (a built binary, a fixture) — `prova.root` is the *consuming*
-- package's root, which is wrong the moment this package is reused cross-repo via
-- `[dependencies] x = { path = … }`.
M.own_dir = pkg.dir

-- The deprecated alias, captured so the proof can pin that both names answer until 1.0.
M.own_dir_via_alias = plugin.dir

return M
