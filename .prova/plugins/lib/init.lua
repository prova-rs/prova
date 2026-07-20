-- A library plugin that privately depends on `inner`. It composes inner's value into its OWN surface
-- (`derived`) — proving a library CAN use a dependency internally — while never re-exporting `inner`.
-- A consumer that requires `lib` must get `lib`, and NOT `inner`.
--
-- Today `require("inner")` resolves via the global `.prova/plugins` disk root (the leak the isolation
-- proof pins). Under the bundled+isolated model, this require binds to lib's own dependency map
-- (`prova-plugin.toml [plugins]`) via a plugin-scoped `require`, so `inner` resolves privately for lib
-- and stops resolving for any consumer.
local inner = require("inner")

local M = {}

M.derived = inner.stamp(inner.secret) -- "inner-secret::stamped-by-inner"

return M
