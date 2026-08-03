-- The out-of-the-box `shared` plugin: require("shared"). It registers fixtures (side effect) and
-- returns typed handles + helpers. Each suite that requires it builds its OWN instances (shared
-- recipe, isolated per suite). Delete this plugin if you don't want it; lift it to a git repo and
-- declare it in [plugins] to share across projects — the `require("shared")` never changes.
local M = {}

-- A shared fixture, used by handle: `local S = require("shared"); t:use(S.greeting)`.
M.greeting = prova.fixture("greeting", Scope.Test, function()
  return "hello from the shared plugin"
end)

-- A plain helper — plugins are just libraries.
function M.slugify(s)
  return (s:lower():gsub("%s+", "-"))
end

-- Where this plugin's own file lives, from the per-plugin `plugin` table. A plugin uses this to find
-- ITS OWN repo's artifacts (a built binary, a fixture) — `prova.root` is the *consuming* package's
-- root, which is wrong the moment this plugin is reused cross-repo via `[plugins] x = { path = … }`.
M.own_dir = plugin.dir

return M
