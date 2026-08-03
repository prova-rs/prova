--- Black-box surface of the suite model — the grouping rules and scope lifetimes that
--- `proofs/suites/` demonstrates from the inside, pinned here from the outside.
---
--- The contract (docs/design/suites.md): a file in no declared suite is its own one-file suite
--- (so `Scope.Suite` == `Scope.File` there, correctly); a directory's `suite.lua` declares a
--- suite of its subtree's test files, runs once and first in the suite's state, and a nested
--- `suite.lua` starts a new suite; the manifest's `[suites.<name>]` does the same for groupings
--- that cross the directory tree; and an unmet suite `requires` skips every file in the suite.

--- A fresh empty directory per call; each test builds its own package in one.
local scratch = prova.fixture("suite-model-scratch", Scope.File, function(ctx)
  return function() return ctx:tempdir() end
end)

local function run(dir, args)
  return shell.run(prova.bin .. (args and (" " .. args) or ""), {
    cwd = dir,
    merge_stderr = true,
  })
end

--- Occurrences of a marker in an output stream.
local function count(out, marker)
  local n = 0
  for _ in out:gmatch(marker) do n = n + 1 end
  return n
end

-- ── ungrouped files are singleton suites ─────────────────────────────────────────────────────

prova.test("an ungrouped file is its own suite: Scope.Suite builds per file, and that is correct",
  { covers = "docs/design/suites.md#singleton-suite-compat" }, function(t)
  -- Two files, no suite.lua, both using a Scope.Suite fixture of the same name. Each file is a
  -- one-file suite, so the factory runs once PER FILE — the suite *is* the file, not a lie about
  -- sharing that never happens.
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  for _, name in ipairs({ "a", "b" }) do
    fs.write(root .. "/proofs/" .. name .. "_test.lua", [[
prova.fixture("res", Scope.Suite, function(ctx) print("RES-BUILT") return {} end)
prova.test("uses the fixture", function(t) t:use("res") t:expect(1):equals(1) end)
]])
  end

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(count(r.stdout, "RES%-BUILT"), "one build per singleton suite"):equals(2)
end)

-- ── the suite.lua convention ─────────────────────────────────────────────────────────────────

--- One declared suite: a setup file, two members sharing a live value, per-file and per-suite
--- markers printed by the factories so lifetimes are countable from the outside.
local function declared_suite(root)
  fs.mkdir(root .. "/proofs/api")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/api/suite.lua", [[
print("SETUP-RAN")
suite.config{ name = "api" }
prova.fixture("store", Scope.Suite, function(ctx)
  print("SUITE-BUILT")
  ctx:defer(function() print("SUITE-TORNDOWN") end)
  return { rows = {} }
end)
prova.fixture("perfile", Scope.File, function(ctx) print("FILE-BUILT") return {} end)
]])
  fs.write(root .. "/proofs/api/a_test.lua", [[
prova.test("a writes", function(t)
  t:use("perfile")
  t:use("store").rows[1] = "from-a"
  t:expect(1):equals(1)
end)
]])
  fs.write(root .. "/proofs/api/b_test.lua", [[
prova.test("b reads what a wrote", function(t)
  t:use("perfile")
  t:expect(t:use("store").rows[1]):equals("from-a")
end)
]])
end

prova.test("a directory's suite.lua declares the suite: setup runs first, members share the state",
  { covers = "docs/design/suites.md#suite-lua-convention" }, function(t)
  local root = t:use(scratch)()
  declared_suite(root)

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the second file sees the first file's write"):contains("b reads what a wrote")
  t:expect(r.stdout:find("SETUP%-RAN") < r.stdout:find("PASS"),
    "the setup file ran before any test"):equals(true)
end)

prova.test("a nested suite.lua ends the parent's reach and starts its own suite",
  { covers = "docs/design/suites.md#suite-lua-convention" }, function(t)
  local root = t:use(scratch)()
  declared_suite(root)
  -- A nested suite declaring a fixture of the SAME name: its member must get the nested suite's
  -- fresh instance, never the parent's shared one.
  fs.mkdir(root .. "/proofs/api/inner")
  fs.write(root .. "/proofs/api/inner/suite.lua", [[
suite.config{ name = "inner" }
prova.fixture("store", Scope.Suite, function(ctx) print("INNER-BUILT") return { rows = {} } end)
]])
  fs.write(root .. "/proofs/api/inner/c_test.lua", [[
prova.test("c gets its own suite's store, not the parent's", function(t)
  t:expect(t:use("store").rows[1]):equals(nil)
end)
]])

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("c gets its own suite's store")
  t:expect(count(r.stdout, "INNER%-BUILT"), "the nested suite built its own"):equals(1)
end)

-- ── scope lifetimes inside a multi-file suite ────────────────────────────────────────────────

prova.test("in a two-file suite: Scope.Suite builds once, Scope.File per file, teardown once at the end",
  { covers = "docs/design/suites.md#suite-scope-semantics" }, function(t)
  local root = t:use(scratch)()
  declared_suite(root)

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(count(r.stdout, "SUITE%-BUILT"), "one live value for the whole suite"):equals(1)
  t:expect(count(r.stdout, "FILE%-BUILT"), "the file scope resets at each file boundary"):equals(2)
  t:expect(count(r.stdout, "SUITE%-TORNDOWN"), "torn down once"):equals(1)
  t:expect(r.stdout:find("SUITE%-TORNDOWN") > r.stdout:find("b reads what a wrote"),
    "teardown fires after the suite's last test"):equals(true)
end)

-- ── the manifest's explicit, cross-cutting suites ────────────────────────────────────────────

prova.test("[suites.<name>] groups paths the directory tree doesn't, sharing one setup and state",
  { covers = "docs/design/suites.md#manifest-suites-explicit" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/svc/one")
  fs.mkdir(root .. "/svc/two")
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[suites.crosscut]
paths = ["svc/one", "svc/two"]
setup = "svc/shared_suite.lua"
]])
  fs.write(root .. "/svc/shared_suite.lua", [[
suite.config{ name = "crosscut" }
prova.fixture("store", Scope.Suite, function(ctx) print("XCUT-BUILT") return { rows = {} } end)
]])
  fs.write(root .. "/svc/one/a_test.lua",
    'prova.test("one writes", function(t) t:use("store").rows[1] = "x" t:expect(1):equals(1) end)\n')
  fs.write(root .. "/svc/two/b_test.lua",
    'prova.test("two reads", function(t) t:expect(t:use("store").rows[1]):equals("x") end)\n')

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "state crossed the directory boundary"):contains("two reads")
  t:expect(count(r.stdout, "XCUT%-BUILT"), "one instance across both paths"):equals(1)
end)

-- ── an unmet suite requirement cascades ──────────────────────────────────────────────────────

prova.test("a suite requiring an unmet capability skips every one of its files",
  { covers = "docs/design/suites.md#suite-requires-cascade" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs/gated")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/gated/suite.lua",
    'suite.config{ name = "gated", requires = { "no-such-capability" } }\n')
  fs.write(root .. "/proofs/gated/g1_test.lua",
    'prova.test("g1", function(t) t:expect(1):equals(1) end)\n')
  fs.write(root .. "/proofs/gated/g2_test.lua",
    'prova.test("g2", function(t) t:expect(1):equals(1) end)\n')

  local r = run(root)
  t:expect(r.code, "skipped is not failed"):equals(0)
  t:expect(r.stdout):contains("0 passed")
  t:expect(r.stdout, "both files skipped"):contains("2 skipped")
  t:expect(r.stdout, "each skip names the unmet requirement"):contains("no-such-capability")
  t:expect(r.stdout, "nothing in the suite ran"):never():contains("PASS")
end)
