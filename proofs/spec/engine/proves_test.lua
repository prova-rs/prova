--- The `proves` attribute — graduation keeps its context.
---
--- A spec flag carries the WHY while the proof is red; graduation used to force bare deletion,
--- which kept the ceremony honest but *discarded the context* the moment it was earned. The
--- revision: an honored spec is converted to `proves = "<context>"` (preferred) or removed.
--- `proves` is inert — the test is a full proof — but the design context now lives in the test
--- itself, where an agent reviewing it cannot miss it. Deliberately NOT a reference system
--- (no "see docs/foo.md"): a pointer is easy to ignore and easier to let drift; prose in the
--- opts table travels with the assertions it explains. A finished test can also be retrofitted
--- with `proves` to capture context after the fact.
---
--- Contract under proof here:
---   * `proves = "<context>"` on a test/flow is runtime-inert: pass is PASS, fail is FAIL.
---   * an honored spec's failure message offers the conversion, reason carried over verbatim.
---   * `spec` + `proves` on one test is refused — open work keeps its context in `spec`.
---   * `proves` demands a non-empty string: the context IS the point; a bare flag says nothing.
---   * test-level only, like `spec`: no group/suite inheritance ceremony.
---   * `prova specs` still enumerates only the OPEN surface — proves-annotated tests are done.

local SPEC = "proves attribute: graduation keeps its context"

-- A fresh child package per call, probed through the real `prova` binary.
local function pkg(body)
  local dir = fs.tempdir() .. "/pkg"
  shell.run("mkdir -p " .. dir .. "/proofs", { check = true })
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(dir .. "/proofs/subject_test.lua", body)
  return dir
end

prova.test("a proves-annotated test is a plain full proof — the attribute is inert",
  { spec = SPEC }, function(t)
  local proj = pkg(
    'prova.test("holds", { proves = "context lives here" }, function(t)\n' ..
    '  t:expect(1 + 1):equals(2)\n' ..
    'end)\n')
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 passed")
  t:expect(r.stdout):never():contains("spec open")      -- proves is not the spec outcome
end)

prova.test("a proves-annotated test that fails is a real failure — no inversion",
  { spec = SPEC }, function(t)
  local proj = pkg(
    'prova.test("broken", { proves = "context lives here" }, function(t)\n' ..
    '  t:expect(1):equals(2)\n' ..
    'end)\n')
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("1 failed")
end)

prova.test("an honored spec offers the conversion, its reason carried into the fix",
  { spec = SPEC }, function(t)
  local proj = pkg(
    'prova.test("done", { spec = "why this matters" }, function(t)\n' ..
    '  t:expect(true):is_true()\n' ..
    'end)\n')
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)                    -- graduation is still mandatory
  t:expect(r.stdout):contains("spec honored")
  t:expect(r.stdout):contains('proves = "why this matters"')
end)

prova.test("spec and proves on one test are refused — not both", { spec = SPEC }, function(t)
  local proj = pkg(
    'prova.test("confused", { spec = "open", proves = "done" }, function(t)\n' ..
    '  t:expect(true):is_true()\n' ..
    'end)\n')
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out):contains("not both")
end)

prova.test("proves demands its context — a bare or empty flag is refused",
  { spec = SPEC }, function(t)
  for _, value in ipairs({ "true", '""' }) do
    local proj = pkg(
      'prova.test("silent", { proves = ' .. value .. ' }, function(t)\n' ..
      '  t:expect(true):is_true()\n' ..
      'end)\n')
    local r = shell.run("prova 2>&1", { cwd = proj })
    t:expect(r.code):never():equals(0)
    local out = r.stdout .. r.stderr
    t:expect(out):contains("proves")
    t:expect(out):contains("context")
  end
end)

prova.test("proves is test-level only — a group-level attribute is refused",
  { spec = SPEC }, function(t)
  local proj = pkg(
    'prova.group("g", { proves = "context" }, function(g)\n' ..
    '  g:test("inside", function(t) t:expect(true):is_true() end)\n' ..
    'end)\n')
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out):contains("proves is test-level only")
end)

prova.test("`prova specs` enumerates only the open surface — proven tests are done",
  { spec = SPEC }, function(t)
  local proj = pkg(
    'prova.test("finished", { proves = "context lives here" }, function(t)\n' ..
    '  t:expect(true):is_true()\n' ..
    'end)\n' ..
    'prova.test("still open", { spec = "not built yet" }, function(t)\n' ..
    '  t:expect(1):equals(2)\n' ..
    'end)\n')
  local r = shell.run("prova specs 2>&1", { cwd = proj })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("still open")
  t:expect(r.stdout):never():contains("finished")
end)

prova.test("the binary teaches the lifecycle: `prova learn specs` names proves",
  { spec = SPEC }, function(t)
  local r = shell.run("prova learn specs 2>&1")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("proves")
end)
