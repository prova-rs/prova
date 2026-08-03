-- The test-door half of the warm fixture: the SAME factory `[topologies]` registers for `up`,
-- declared here as an ordinary fixture. Under a warm run, `t:use(warm)` resolves the server's
-- HELD instance by name instead of provisioning.

local warm = prova.topology("warmtop", require("warmtop").make)

prova.test("warm state accumulates across runs", function(t)
  local env = t:use(warm)
  env.counter.hits = env.counter.hits + 1
  fs.write("hits", tostring(env.counter.hits))
  t:expect(env.counter.hits):gte(1)
end)
