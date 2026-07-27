--- The globals injection contract (docs/plans/api-freeze.md §2) — three mechanisms close every
--- silent-collision path between bundled namespaces, plugins, and user code:
---
---   1. reserved-name registry — a `[plugins]` entry or plugin-root file bearing a bundled
---      namespace name is a MANIFEST VALIDATION error, never a silent shadow in either direction.
---   2. write-protected globals — assignment to a reserved global raises with guidance; local
---      shadowing (`local fs = ...`) stays legal and untouched — lexical, visible, deliberate.
---   3. injection over `require` — every bundled namespace is require-able by name (the
---      searcher's bundled tier); global injection is sugar, removable per-name via
---      `[run] globals = { exclude = [...] }`. Default remains all-injected.
---
--- Probes that could clobber the parent's own globals while the protection is unbuilt run in a
--- sandbox child package (the spec-engine idiom), not in-process.

local sandbox = prova.fixture("globals-sandbox", Scope.File, function(ctx)
  return ctx:tempdir()
end)

local function child(t, name, manifest, test_body)
  local proj = t:use(sandbox) .. "/" .. name
  shell.run("mkdir -p " .. proj .. "/proofs", { check = true })
  fs.write(proj .. "/prova.toml", manifest)
  if test_body then
    fs.write(proj .. "/proofs/probe_test.lua", test_body)
  end
  return proj
end

-- ── 1. the reserved-name registry ────────────────────────────────────────────────────────────

prova.test("a [plugins] entry bearing a reserved name is a manifest validation error",
  { spec = "api-freeze §2: reserved-name registry — not built" }, function(t)
  local proj = child(t, "reserved-plugin",
    '[run]\nproofs = ["proofs"]\n\n[plugins]\nfs = "./fsplug"\n',
    'prova.test("never runs", function(t) t:expect(true):is_true() end)\n')
  shell.run("mkdir -p " .. proj .. "/fsplug", { check = true })
  fs.write(proj .. "/fsplug/init.lua", "return {}\n")

  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)          -- validation error, not a silent shadow
  t:expect(r.stdout):contains("fs")
  t:expect(r.stdout):contains("reserved")     -- the diagnosis names the mechanism
end)

prova.test("a plugin-root file bearing a reserved name is a manifest validation error",
  { spec = "api-freeze §2: reserved-name registry — not built" }, function(t)
  local proj = child(t, "reserved-root",
    '[run]\nproofs = ["proofs"]\nplugin_root = "plugins"\n',
    'prova.test("never runs", function(t) t:expect(true):is_true() end)\n')
  shell.run("mkdir -p " .. proj .. "/plugins", { check = true })
  fs.write(proj .. "/plugins/http.lua", "return {}\n")

  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("http")
  t:expect(r.stdout):contains("reserved")
end)

-- ── 2. write-protected globals ───────────────────────────────────────────────────────────────

prova.test("assignment to a reserved global raises with guidance; local shadowing stays legal",
  { spec = "api-freeze §2: write-protected globals — not built" }, function(t)
  local proj = child(t, "write-protect", '[run]\nproofs = ["proofs"]\n', [[
prova.test("assignment raises, and the error teaches the two outs", function(t)
  local ok, err = pcall(function() fs = {} end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):contains("prova namespace")   -- names the collision
  t:expect(tostring(err)):contains("exclude")           -- points at [run] globals exclusion
end)

prova.test("local shadowing is lexical, visible, deliberate — and untouched", function(t)
  local fs = { marker = true }
  t:expect(fs.marker):is_true()
end)
]])
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):equals(0)
end)

-- ── 3. injection over require ────────────────────────────────────────────────────────────────

prova.test("every bundled namespace is require-able by name — injection is sugar over it",
  { spec = "api-freeze §2: bundled require tier — not built" }, function(t)
  local m = require("fs")
  t:expect(type(m.write)):equals("function")
  t:expect(m == fs):is_true()                 -- THE namespace, not a copy
end)

prova.test("[run] globals exclude removes a name from injection; require still reaches it",
  { spec = "api-freeze §2: configurable injection — not built" }, function(t)
  local proj = child(t, "exclude",
    '[run]\nproofs = ["proofs"]\nglobals = { exclude = ["fs"] }\n', [[
prova.test("the excluded name is not injected, but is require-able under any local name", function(t)
  t:expect(fs == nil):is_true()
  local files = require("fs")
  t:expect(type(files.write)):equals("function")
end)
]])
  local r = shell.run("prova 2>&1", { cwd = proj })
  t:expect(r.code):equals(0)
end)
