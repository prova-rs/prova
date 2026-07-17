--- Prova testing Prova: `prova.lua` — the optional project-level companion — and the
--- `prova.capability(name, fn)` registration it exists to host.
---
--- Why a companion rather than `suite.lua`, per docs/design/test-topology.md. Two reasons, and the
--- second is structural rather than a preference:
---
---   1. SCOPE. A capability is a project-wide vocabulary. Registered per-suite it would be invisible
---      to sibling suites and to `must_run` (which lives in prova.toml, project-level), and N copies
---      could silently disagree.
---   2. ORDERING. `must_run` is a PRECONDITION, checked before suites load. A suite-registered
---      capability does not exist yet at that moment — so `must_run = ["gpu"]` could never work. A
---      companion loading with the manifest is what makes it possible at all. The must_run tests
---      below are therefore the real reason this file exists; the rest is the API.
---
--- The predicate is evaluated ONCE, at load, and its answer stored — not called per test. That is
--- not an optimization: a capability that answered differently for two suites in one run would be a
--- bug, and eager evaluation is what lets the precondition see it before any suite exists.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")

--- A predicate body that RECORDS that it ran, then answers `verdict`.
---
--- Load-bearing: without proof of execution, "registered and returned false" is indistinguishable
--- from "capability never registered" — both skip, both name `gpu`, both fail must_run. Every one of
--- those assertions passed against the unimplemented feature until the marker was added.
local function predicate(verdict)
  return 'prova.capability("gpu", function()\n'
      .. '  fs.write(os.getenv("PROVA_SELFTEST_MARK"), "ran")\n'
      .. '  return ' .. verdict .. '\n'
      .. 'end)\n'
end

--- A scratch project. `companion` is written to <home>/prova.lua when given; `manifest_extra` is
--- appended to prova.toml verbatim.
local function project(companion, manifest_extra)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]',
    'paths = ["suite.lua"]',
    '[luals]',
    'manage = "never"',
    manifest_extra or '',
  }, "\n"))
  if companion then fs.write(dir .. "/prova.lua", companion) end
  fs.write(dir .. "/suite.lua", table.concat({
    'prova.test("plain", function(t) t:expect(1):equals(1) end)',
    'prova.test("needs gpu", { requires = { "gpu" } }, function(t) t:expect(1):equals(1) end)',
  }, "\n"))
  return dir
end

------------------------------------------------------------------------------------------
-- A. Loading the companion
------------------------------------------------------------------------------------------

prova.test("no prova.lua behaves exactly as before", function(t)
  -- The companion is OPTIONAL. Absent → today's behavior, unchanged: `gpu` is simply an unknown
  -- capability, so the test that needs it skips.
  local dir = project(nil)
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("plain")
  t:expect(r.stdout, "an unregistered capability is just unavailable"):contains("skipped")
end)

prova.test("a registered capability that holds makes the test RUN", function(t)
  local dir = project(predicate("true"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "gpu is available, so the gated test ran"):contains("2 passed, 0 failed, 0 skipped")
  t:expect(fs.exists(mark), "the predicate ran"):is_true()
end)

prova.test("a registered capability that does NOT hold skips the test", function(t)
  local dir = project(predicate("false"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code, "an unmet requirement is a skip, never a failure"):equals(0)
  -- The marker is what makes this mean anything: it proves the predicate RAN and said no, rather
  -- than the capability having been unknown all along (which skips identically).
  t:expect(fs.exists(mark), "the registered predicate actually ran"):is_true()
  t:expect(r.stdout, "exactly the gated test skipped"):contains("1 passed, 0 failed, 1 skipped")
end)

prova.test("the companion is found next to prova.toml, not in the cwd", function(t)
  -- Home is the anchor: `prova.lua` sits with the manifest it belongs to. Running from a
  -- subdirectory must resolve the same companion — otherwise the project's vocabulary would depend
  -- on where you happened to stand.
  local dir = project(predicate("true"))
  local mark = dir .. "/mark.txt"
  fs.write(dir .. "/sub/keep.txt", "")
  local r = shell.run(prova_bin, { cwd = dir .. "/sub", env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code):equals(0)
  -- "contains 'needs gpu'" would be a false green: the SKIP line names the test too. Demand that it
  -- RAN — both tests passing, nothing skipped.
  t:expect(r.stdout, "the companion resolved from the home, not the cwd"):contains("2 passed, 0 failed, 0 skipped")
  t:expect(fs.exists(mark)):is_true()
end)

prova.test("a broken prova.lua is an ERROR, not a silent skip", function(t)
  -- The failure this closes: a companion that failed to load would leave every capability it meant
  -- to register silently unregistered, so every gated test would skip — and the run would be green.
  -- The vacuous green, one level further out.
  local dir = project('this is not lua((((\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code, "a broken companion is a config error"):equals(2)
  t:expect(r.stderr .. r.stdout):contains("prova.lua")
end)

prova.test("an error raised inside prova.lua is reported", function(t)
  local dir = project('error("companion exploded")\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(2)
  t:expect(r.stderr .. r.stdout):contains("companion exploded")
end)

------------------------------------------------------------------------------------------
-- B. The registration API
------------------------------------------------------------------------------------------

prova.test("the skip reason names the registered capability", function(t)
  local dir = project(predicate("false"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin .. " --format json", { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(fs.exists(mark), "the predicate ran (else this asserts nothing)"):is_true()
  t:expect(r.stdout, "an attributable skip names what was missing"):contains("gpu")
end)

prova.test("the predicate is evaluated ONCE per run, not per test", function(t)
  -- Two gated tests, one predicate. A capability that answered per-test could answer differently
  -- per test, which is not a capability — it is a coin flip. (It also could not be checked as a
  -- precondition, since that happens before any test exists.)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", '[run]\npaths = ["suite.lua"]\n[luals]\nmanage = "never"\n')
  fs.write(dir .. "/prova.lua", table.concat({
    'local n = 0',
    'prova.capability("counted", function()',
    '  n = n + 1',
    '  fs.write(os.getenv("PROVA_SELFTEST_COUNT_FILE"), tostring(n))',
    '  return true',
    'end)',
  }, "\n"))
  fs.write(dir .. "/suite.lua", table.concat({
    'prova.test("a", { requires = { "counted" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("b", { requires = { "counted" } }, function(t) t:expect(1):equals(1) end)',
  }, "\n"))
  local counter = dir .. "/count.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_COUNT_FILE = counter } })
  t:expect(r.code):equals(0)
  t:expect(fs.read(counter), "one evaluation, two consumers"):equals("1")
end)

prova.test("registering over a built-in capability is refused", function(t)
  -- `docker` means something specific (a daemon that answers AND runs linux containers). Letting a
  -- project redefine it would make `requires = { "docker" }` mean different things in different
  -- repos — and silently, which is the worst kind.
  local dir = project('prova.capability("docker", function() return true end)\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(2)
  t:expect(r.stderr .. r.stdout):contains("docker")
end)

------------------------------------------------------------------------------------------
-- C. must_run — the reason the companion is project-level (the ORDERING proof)
------------------------------------------------------------------------------------------

prova.test("must_run can guarantee a REGISTERED capability", function(t)
  -- The structural point. `must_run` is checked before suites load, so this only works because the
  -- companion loads with the MANIFEST. Registered in suite.lua, `gpu` would not exist yet and this
  -- could never pass.
  local dir = project(predicate("true"), '\n[profiles.ci]\nmust_run = ["gpu"]\n')
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code, "a registered capability is guaranteeable"):equals(0)
  t:expect(fs.exists(mark), "the precondition saw a capability the companion registered"):is_true()
end)

prova.test("must_run FAILS when a registered capability does not hold", function(t)
  local dir = project(predicate("false"), '\n[profiles.ci]\nmust_run = ["gpu"]\n')
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code, "an unmet guarantee fails, registered or built-in"):equals(2)
  -- Without the marker this passes against an unimplemented feature: an UNKNOWN gpu also fails.
  t:expect(fs.exists(mark), "it failed because the predicate said no, not because gpu was unknown"):is_true()
  t:expect(r.stderr .. r.stdout):contains("gpu")
end)

prova.test("must_run on an unregistered capability still fails", function(t)
  -- No companion at all: `gpu` is unknown, so the guarantee cannot be honored. A typo'd capability
  -- in must_run must not pass silently.
  local dir = project(nil, '\n[profiles.ci]\nmust_run = ["gpu"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  t:expect(r.code):equals(2)
end)

------------------------------------------------------------------------------------------
-- D. Versions — a predicate may report one
------------------------------------------------------------------------------------------

prova.test("a predicate returning a version satisfies a constraint", function(t)
  -- The registration answers the same two questions a built-in does — present? and which version? —
  -- so a registered capability composes with the same expression grammar rather than being a second
  -- class of thing.
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", '[run]\npaths = ["suite.lua"]\n[luals]\nmanage = "never"\n')
  fs.write(dir .. "/prova.lua", 'prova.capability("gpu", function() return "2.4.0" end)\n')
  fs.write(dir .. "/suite.lua", table.concat({
    'prova.test("ok", { requires = { "gpu >= 2.0" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("too new", { requires = { "gpu >= 9.0" } }, function(t) error("must not run") end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  -- Exact counts: "contains 'ok'" and "contains 'skipped'" both pass when BOTH tests skip, which is
  -- what an unloaded companion produces.
  t:expect(r.stdout, "one ran, one skipped"):contains("1 passed, 0 failed, 1 skipped")
end)

prova.test("a version-reporting predicate's skip says what it found", function(t)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", '[run]\npaths = ["suite.lua"]\n[luals]\nmanage = "never"\n')
  fs.write(dir .. "/prova.lua", 'prova.capability("gpu", function() return "2.4.0" end)\n')
  fs.write(dir .. "/suite.lua",
    'prova.test("too new", { requires = { "gpu >= 9.0" } }, function(t) error("must not run") end)')
  local r = shell.run(prova_bin .. " --format json", { cwd = dir })
  t:expect(r.stdout, "reports the found version, not just the constraint"):contains("2.4.0")
end)
