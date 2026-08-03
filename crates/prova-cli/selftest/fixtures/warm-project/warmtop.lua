-- The warm-phase observable factory: provisioning and teardown leave file sentinels, and the
-- value is a mutable Lua table — so a proof can distinguish "resolved the held instance" from
-- "re-provisioned a fresh one" across separate MCP tool calls. Exported from a plugin so BOTH
-- doors reach one definition: `[topologies]` registers it for `up`, and the test file declares
-- it as a fixture (see docs/design/topologies.md §Two doors).
local M = {}

function M.make(ctx)
  local n = (fs.exists("provisions") and tonumber(fs.read("provisions")) or 0) + 1
  fs.write("provisions", tostring(n))
  ctx:defer(function() fs.write("teardown", "done") end)
  return { counter = { hits = 0 }, url = "mem://warmtop" }
end

return M
