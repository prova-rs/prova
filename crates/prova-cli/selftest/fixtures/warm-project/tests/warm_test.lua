-- The warm-phase observable fixture: a topology whose provisioning and teardown leave file
-- sentinels, and whose held value is a mutable Lua table — so a proof can distinguish "resolved
-- the held instance" from "re-provisioned a fresh one" across separate MCP tool calls.

local warm = prova.topology("warmtop", function(ctx)
  local n = (fs.exists("provisions") and tonumber(fs.read("provisions")) or 0) + 1
  fs.write("provisions", tostring(n))
  ctx:defer(function() fs.write("teardown", "done") end)
  return { counter = { hits = 0 }, url = "mem://warmtop" }
end)

prova.test("warm state accumulates across runs", function(t)
  local env = t:use(warm)
  env.counter.hits = env.counter.hits + 1
  fs.write("hits", tostring(env.counter.hits))
  t:expect(env.counter.hits):gte(1)
end)
