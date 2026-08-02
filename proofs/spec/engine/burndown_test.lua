--- Black-box surface of the spec engine itself, driven through a sandbox child package that
--- carries one normal test, one open spec, and one honored spec. Two layers under proof here:
---
---   primitives : `--promises` the composable selector, `--promises --list` enumeration —
---                bootstrapped without proofs ("implemented first, spec'd by hand"); the
---                guardrail below closes that gap.
---   verbs      : `prova promises` and `prova burndown` — the memorable entry points, subsuming
---                `--promises --list` / `--promises --due`. Activities are subcommands in
---                prova's grammar (`prova up`, `prova plugin`), and no-arg subcommands list
---                their domain; the spec lifecycle gets the same ergonomics. An empty surface
---                under `burndown` means COMPLETE (exit 0), not a selection error.

local sandbox = prova.fixture("spec-engine-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/widget_test.lua", [[
prova.test("arithmetic holds", function(t)
  t:expect(1 + 1):equals(2)
end)

prova.test("the widget frobnicates", { promises = "sandbox: not built yet" }, function(t)
  t:expect(1):equals(2)
end)

prova.test("the widget already exists", { promises = "sandbox: already true" }, function(t)
  t:expect(true):is_true()
end)
]])
  return proj
end)

-- ── the primitive, proven (guardrail — this works today and must keep working) ───────────────

prova.test("`prova --promises --list` enumerates the open surface without running anything",
  function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " --promises --list", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("frobnicates")            -- both flagged tests are the surface
  t:expect(r.stdout):contains("already exists")
  t:expect(r.stdout):never():contains("arithmetic")     -- unflagged tests are not specs
  t:expect(r.stdout):never():contains("passed")         -- enumeration only — no run, no tally
end)

-- ── the verbs, spec'd ────────────────────────────────────────────────────────────────────────

prova.test("`prova promises` enumerates the open surface — the no-flags spelling", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " promises", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("frobnicates")
  t:expect(r.stdout):contains("already exists")
  t:expect(r.stdout):never():contains("arithmetic")
end)

prova.test("`prova burndown` is the inner loop: spec-selected, open specs fail loud", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " burndown", { cwd = proj, merge_stderr = true })
  t:expect(r.code):never():equals(0)                    -- open specs are real failures here
  t:expect(r.stdout):contains("frobnicates")            -- the open spec, with its detail
  t:expect(r.stdout):contains("expected")               -- full failure detail, not a summary
  t:expect(r.stdout):contains("promise kept")           -- the kept promise demands graduation
  t:expect(r.stdout):never():contains("arithmetic holds")  -- unflagged tests are not selected
end)

prova.test("the binary teaches the verbs: `prova learn promises` names them", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " learn promises", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("prova promises")
  t:expect(r.stdout):contains("prova burndown")
end)
