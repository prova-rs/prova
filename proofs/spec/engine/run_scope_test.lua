--- Scope.Run — the fifth scope (docs/plans/shared-deputies.md): one instance across every suite
--- and worker, resolved through the run-wide conduct store. Lazy, blocking, single-flight —
--- whichever consumer asks first conducts, everyone else waits for the settled slot, and waiting
--- IS the ordering. Values are DATA (JSON-serializable, copied per state); failure memoizes for
--- the whole run exactly as it does per scope instance everywhere else.
---
--- Every scenario builds a package whose two proof FILES are two singleton suites — two Lua
--- states, and under `-j 2` two workers — the exact boundary a Lua value cannot cross and this
--- scope exists to.

--- A package with a `deputies` local package (the blessed recipe-sharing shape) and two
--- singleton-suite proof files. `factory` is the deputy's body; each proof file gets `body`.
local function two_suites(t, factory, body)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/packages/deputies")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\npackages = "packages"\n')
  fs.write(proj .. "/packages/deputies/prova.toml", '[package]\nname = "deputies"\n')
  fs.write(proj .. "/packages/deputies/init.lua", [[
local M = {}
M.conduct = prova.fixture("shared-conduct", Scope.Run, ]] .. factory .. [[)
return M
]])
  for _, name in ipairs({ "alpha", "beta" }) do
    fs.write(proj .. "/proofs/" .. name .. "_test.lua",
      'local d = require("deputies")\n' .. body)
  end
  return proj
end

local COUNTING = [[function()
  local counter = os.getenv("CONDUCT_COUNT")
  local n = fs.exists(counter) and tonumber(fs.read(counter)) or 0
  fs.write(counter, tostring(n + 1))
  return { artifact = "junit-" .. tostring(n + 1) .. ".xml" }
end]]

prova.test("two suites on two workers conduct once and read one value", {
  covers = "docs/design/verifiers.md#suite-scoped-shared-deputies",
  proves = "the suite boundary is a Lua-state boundary a value cannot cross, so conduct-once-read-many used to stop there: the ut lane's workspace conduct could not feed a reader in another directory without paying a second cargo. One slot per name, run-wide, is the crossing",
}, function(t)
  local proj = two_suites(t, COUNTING, [[
prova.test("reads the shared conduct", function(t)
  t:expect(t:use(d.conduct).artifact, "every reader sees the ONE conduct"):equals("junit-1.xml")
end)
]])
  local counter = proj .. "/count.txt"
  local r = shell.run(prova.bin .. " -j 2", { cwd = proj, merge_stderr = true, env = { CONDUCT_COUNT = counter } })

  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("2 passed")
  t:expect(fs.read(counter), "the factory ran exactly once across both workers"):equals("1")
end)

prova.test("a poisoned conduct fails every suite with the one recorded error", {
  covers = "docs/design/verifiers.md#suite-scoped-shared-deputies",
  proves = "the run-instance form of fixture-failure memoization: one dead cargo is one payment even when the readers live in different Lua states — and the replay names itself, so a memoized verdict can never read as a fresh attempt",
}, function(t)
  local proj = two_suites(t, [[function()
  local counter = os.getenv("CONDUCT_COUNT")
  local n = fs.exists(counter) and tonumber(fs.read(counter)) or 0
  fs.write(counter, tostring(n + 1))
  error("the deputy died")
end]], [[
prova.test("wants the conduct", function(t)
  t:use(d.conduct)
end)
]])
  local counter = proj .. "/count.txt"
  local r = shell.run(prova.bin .. " -j 2", { cwd = proj, merge_stderr = true, env = { CONDUCT_COUNT = counter } })

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("2 failed")
  t:expect(fs.read(counter), "one payment"):equals("1")
  t:expect(r.stdout, "the original error travels"):contains("the deputy died")
  t:expect(r.stdout, "the replay names itself"):contains("memoized, not re-provisioned")
end)

prova.test("a run-scoped value must be plain data, and the constraint is taught at the boundary", {
  covers = "docs/design/verifiers.md#suite-scoped-shared-deputies",
  proves = "the honest cost of run scope, stated once, where it bites: a Lua function cannot cross states, so returning one must error naming the constraint — never serialize to garbage a reader trips over later",
}, function(t)
  local proj = two_suites(t, [[function()
  return { helper = function() end }
end]], [[
prova.test("wants the conduct", function(t)
  t:use(d.conduct)
end)
]])
  local r = shell.run(prova.bin .. " -j 2", { cwd = proj, merge_stderr = true, env = { CONDUCT_COUNT = proj .. "/c.txt" } })

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout, "the constraint is named"):contains("plain data")
  t:expect(r.stdout):contains("JSON-serializable")
end)

prova.test("a run-scoped factory has no ctx:defer — the refusal teaches where artifacts live", {
  covers = "docs/design/verifiers.md#suite-scoped-shared-deputies",
  proves = "a defer registered in one Lua state cannot run after that state is gone, and a teardown that silently never fires is worse than none — refuse at registration, naming the doctrine (artifacts live in the tree)",
}, function(t)
  local proj = two_suites(t, [[function(ctx)
  ctx:defer(function() end)
  return "x"
end]], [[
prova.test("wants the conduct", function(t)
  t:use(d.conduct)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true, env = { CONDUCT_COUNT = proj .. "/c.txt" } })

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("no ctx:defer")
end)

prova.test("a same-state waiter never wedges the thread driving the conduct it waits on", {
  covers = "docs/design/verifiers.md#suite-scoped-shared-deputies",
  proves = "two leaves of ONE suite share one thread: if the second blocked the thread while the first's factory awaited its shell.run, the run would deadlock — the wait must yield to the runtime it shares",
}, function(t)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/one_test.lua", [[
local slow = prova.fixture("slow-conduct", Scope.Run, function()
  shell.run("sleep 0.3")
  return "ready"
end)
prova.test("first consumer", function(t) t:expect(t:use(slow)):equals("ready") end)
prova.test("second consumer", function(t) t:expect(t:use(slow)):equals("ready") end)
]])
  -- The outer bound turns a deadlock into a bounded, named red instead of a hung suite.
  local r = shell.run(prova.bin .. " -j 2", { cwd = proj, merge_stderr = true, timeout = "30s" })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("2 passed")
end)
