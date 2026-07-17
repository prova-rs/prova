--- Prova testing Prova: the skip/fail contract from docs/design/test-topology.md.
---
--- The principle under test:
---   A pass is a claim about the CODE. A skip is a claim about the ENVIRONMENT.
---   Never let the second masquerade as the first.
---
--- A skipped test is an unanswered question, not a passed one. `requires` states a test's NEED (a
--- portable fact about the test); a profile's `must_run` states the environment's GUARANTEE (policy,
--- which changes when you move). A guaranteed capability that is absent is a broken environment —
--- so it FAILS, and it fails as a PRECONDITION, before any test runs.
---
--- Exit codes (established by manifest_test.lua): 0 = pass, 1 = a test failed, 2 = usage/config
--- error. A must_run violation is a 2: no test failed — the environment cannot honor the manifest.
--- That distinction is worth keeping, because "tests failed" and "your runner is broken" want
--- different responses from whoever is paged.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")

-- A capability that cannot exist on any machine: the PATH-probe fallback will never find it.
local ABSENT = "prova-definitely-not-a-real-tool-xyz"
-- A capability present on every machine prova's CI runs (POSIX + Windows runners all have it on
-- PATH). `sh` is the safest universally-present binary name.
local PRESENT = "sh"

--- A scratch project. `manifest_extra` is appended verbatim so each test declares its own profiles.
local function project(manifest_extra)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]',
    'paths = ["suite.lua"]',
    '[luals]',
    'manage = "never"',
    manifest_extra or '',
  }, "\n"))
  fs.write(dir .. "/suite.lua", table.concat({
    'prova.test("plain", function(t) t:expect(1):equals(1) end)',
    'prova.test("needs absent", { requires = { "' .. ABSENT .. '" } }, function(t)',
    '  t:expect(1):equals(1)',
    'end)',
    'prova.test("tagged", { tags = { "slow" } }, function(t) t:expect(1):equals(1) end)',
  }, "\n"))
  return dir
end

------------------------------------------------------------------------------------------
-- A. must_run — the precondition
------------------------------------------------------------------------------------------

prova.test("must_run with a PRESENT capability runs normally", function(t)
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. PRESENT .. '"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  t:expect(r.code, "a satisfied guarantee changes nothing"):equals(0)
  t:expect(r.stdout):contains("plain")
end)

prova.test("must_run with an ABSENT capability fails the run", function(t)
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. ABSENT .. '"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  -- The whole point: this must NOT be a green run with a quiet skip.
  t:expect(r.code, "an unmet guarantee is a failure, not a skip"):never():equals(0)
  t:expect(r.code, "…and it is an environment error (2), not a test failure (1)"):equals(2)
end)

prova.test("the must_run failure names the capability", function(t)
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. ABSENT .. '"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  local out = r.stderr .. r.stdout
  t:expect(out, "names the capability"):contains(ABSENT)
  t:expect(out, "names the profile that guaranteed it"):contains("ci")
end)

prova.test("must_run is a PRECONDITION: no test runs when it is unmet", function(t)
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. ABSENT .. '"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  -- Fail fast: the point of a precondition over a post-hoc skip-audit is that you learn at second
  -- one, not after a suite has run. `plain` requires nothing and would otherwise have passed.
  t:expect(r.stdout, "no test executed"):never():contains("plain")
end)

prova.test("must_run is generic over the capability vocabulary, not docker-special", function(t)
  -- `requires` already falls through to a binary-on-PATH probe, so must_run needs no new detector:
  -- any tool name works, and the two use ONE vocabulary.
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. PRESENT .. '", "' .. ABSENT .. '"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  local out = r.stderr .. r.stdout
  t:expect(r.code):equals(2)
  t:expect(out, "names the MISSING one"):contains(ABSENT)
  t:expect(out, "does not blame the present one"):never():contains('"' .. PRESENT .. '" is unavailable')
end)

prova.test("must_run applies only to the profile that declares it", function(t)
  -- The default run must be untouched by a ci-only guarantee: policy belongs to the context.
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. ABSENT .. '"]\n')
  local r = shell.run(prova_bin, { cwd = dir })   -- no --profile
  t:expect(r.code, "the default profile made no such promise"):equals(0)
  t:expect(r.stdout):contains("plain")
end)

prova.test("without must_run, an unmet `requires` still skips — existing suites unchanged", function(t)
  local dir = project()                            -- no profiles at all
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code, "a skip is not a failure by default"):equals(0)
  t:expect(r.stdout, "and it is reported, not hidden"):contains("skipped")
end)

prova.test("a skip is attributable: the report says WHICH capability was missing", function(t)
  -- A skip is an unanswered question; an unattributed skip is an unanswered question you cannot even
  -- name. Whatever the format, the missing capability has to be recoverable from the output.
  local dir = project()
  local r = shell.run(prova_bin .. " --format json", { cwd = dir })
  t:expect(r.stdout, "the skip carries its reason"):contains(ABSENT)
end)

------------------------------------------------------------------------------------------
-- B. Empty selection — the same principle, on the other axis
------------------------------------------------------------------------------------------
-- Selection is INTENT ("run less"); capability is ABILITY ("cannot run here"). Both end in "did not
-- run", which is why they are confused. But a selection that matches NOTHING is nearly always a
-- typo, and a typo must not be green.

prova.test("a selection matching nothing is an error, not a green run", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " -k thisdoesnotexistanywhere", { cwd = dir })
  t:expect(r.code, "asking nothing is not success"):never():equals(0)
end)

prova.test("the empty-selection error says what matched nothing", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " -k thisdoesnotexistanywhere", { cwd = dir })
  local out = r.stderr .. r.stdout
  t:expect(out):contains("thisdoesnotexistanywhere")
end)

prova.test("--allow-empty opts out, for the matrix that legitimately selects nothing", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " -k thisdoesnotexistanywhere --allow-empty", { cwd = dir })
  t:expect(r.code):equals(0)
end)

prova.test("a selection that DOES match is unaffected", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " -k plain", { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("plain")
end)

prova.test("an empty --tags selection errors the same way", function(t)
  -- Same rule, a different selector: the axis is what matters, not the flag.
  local dir = project()
  local r = shell.run(prova_bin .. " --tags nosuchtag", { cwd = dir })
  t:expect(r.code):never():equals(0)
end)

prova.test("a tag that DOES match is unaffected", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " --tags slow", { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("tagged")
end)

------------------------------------------------------------------------------------------
-- C. The interaction — selection and capability are ORTHOGONAL
------------------------------------------------------------------------------------------

prova.test("must_run is not satisfied by deselecting the tests that need it", function(t)
  -- The subtle one. `must_run` is a statement about the ENVIRONMENT, so it holds regardless of what
  -- this invocation happened to select. If deselecting could satisfy a guarantee, `-k` would become
  -- a way to silence the contract — the exact escape hatch that makes a bar meaningless.
  local dir = project('\n[profiles.ci]\nmust_run = ["' .. ABSENT .. '"]\n')
  local r = shell.run(prova_bin .. " --profile ci -k plain", { cwd = dir })
  t:expect(r.code, "the guarantee is about the machine, not the selection"):equals(2)
end)
