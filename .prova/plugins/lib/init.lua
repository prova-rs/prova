-- A library plugin that privately depends on `inner`. It composes inner's value into its OWN surface
-- (`derived`) — proving a library CAN use a dependency internally — while never re-exporting `inner`.
-- A consumer that requires `lib` must get `lib`, and NOT `inner`.
--
-- This `require` binds to lib's own dependency map (`prova.toml [plugins]`) through a
-- plugin-scoped `require`, so `inner` resolves here and nowhere else. Nothing about this line looks
-- special — that is the point: a plugin author writes an ordinary require and gets privacy by
-- declaring the dependency, not by using a different API.
local inner = require("inner")

local M = {}

M.derived = inner.stamp(inner.secret) -- "inner-secret::stamped-by-inner"

return M
