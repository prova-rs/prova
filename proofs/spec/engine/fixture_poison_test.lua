--- Fixture failure memoization (docs/design/lifecycle.md#fixture-failure-memoization): a factory
--- runs at most once per scope instance, whatever the outcome. Success was always cached; failure
--- is now recorded the same way, and every later consumer in the scope instance is poisoned with
--- the one recorded error — named as a memoized replay, never re-paid. The field report that
--- filed it: a file-scoped nextest conduct hit its timeout and five readers re-paid 5 × 600s.

local scaffold = require("scaffold")

local function package(t, proof)
  return scaffold.package(t, { proofs = { ["poison_test.lua"] = proof } })
end

prova.test("a failed file-scoped fixture provisions once; readers replay the recorded error", {
  covers = "docs/design/lifecycle.md#fixture-failure-memoization",
  proves = "re-provisioning a failed conduct has no upside at file scope — nothing changed between attempts except the clock — and it multiplies the cost by the reader count: five dependents of one timed-out cargo conduct re-paid 5 × 600s before the run ended",
}, function(t)
  local proj = package(t, [[
local attempts = os.getenv("POISON_ATTEMPTS")
local broken = prova.fixture("broken-conduct", Scope.File, function(ctx)
  local n = fs.exists(attempts) and tonumber(fs.read(attempts)) or 0
  fs.write(attempts, tostring(n + 1))
  error("the conduct timed out")
end)
for i = 1, 5 do
  prova.test("reader " .. i, function(t)
    t:use(broken)
  end)
end
]])
  local attempts = proj .. "/attempts.txt"
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true, env = { POISON_ATTEMPTS = attempts } })

  t:expect(r.code, "the readers all fail — poison is not a pass"):never():equals(0)
  t:expect(r.stdout):contains("5 failed")
  t:expect(fs.read(attempts), "the factory ran exactly once"):equals("1")
  t:expect(r.stdout, "the replay names itself — a memoized verdict must never read as a fresh attempt")
    :contains("memoized")
  t:expect(r.stdout, "the recorded error travels to every reader"):contains("the conduct timed out")
end)

prova.test("poison lives exactly as long as a cached success would: per scope instance", {
  covers = "docs/design/lifecycle.md#fixture-failure-memoization",
  proves = "the memoization domain is the scope instance, same as success — a Scope.Test fixture that fails must still attempt per test (each test is a fresh instance); caching across them would turn one flake into N false failures",
}, function(t)
  local proj = package(t, [[
local attempts = os.getenv("POISON_ATTEMPTS")
local shaky = prova.fixture("per-test", Scope.Test, function(ctx)
  local n = fs.exists(attempts) and tonumber(fs.read(attempts)) or 0
  fs.write(attempts, tostring(n + 1))
  error("fails this test")
end)
for i = 1, 3 do
  prova.test("case " .. i, function(t)
    t:use(shaky)
  end)
end
]])
  local attempts = proj .. "/attempts.txt"
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true, env = { POISON_ATTEMPTS = attempts } })

  t:expect(r.stdout):contains("3 failed")
  t:expect(fs.read(attempts), "a fresh scope instance is a fresh attempt"):equals("3")
end)
