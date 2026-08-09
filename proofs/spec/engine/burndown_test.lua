--- Black-box surface of the spec engine itself, driven through a sandbox child package that
--- carries one normal test, one open promise, and one honored promise. Two layers under proof here:
---
---   primitives : `--promises` the composable selector, `--promises --list` enumeration —
---                bootstrapped without proofs ("implemented first, spec'd by hand"); the
---                guardrail below closes that gap.
---   grammar    : `prova tests --promises` (the report, state-filtered) and `prova tests burndown`
---                (the driver), subsuming `--promises --list` / `--promises --due`. A lane is a
---                noun (`prova tests`), a `--flag` narrows it, a bare word is a driver; an empty
---                surface under `burndown` means COMPLETE (exit 0), not a selection error.

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

prova.test("`prova tests --promises` enumerates the open surface", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " tests --promises", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("frobnicates")
  t:expect(r.stdout):contains("already exists")
  t:expect(r.stdout):never():contains("arithmetic")
end)

prova.test("`prova tests burndown` is the inner loop: promise-selected, open promises fail loud", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " tests burndown", { cwd = proj, merge_stderr = true })
  t:expect(r.code):never():equals(0)                    -- open promises are real failures here
  t:expect(r.stdout):contains("frobnicates")            -- the open promise, with its detail
  t:expect(r.stdout):contains("expected")               -- full failure detail, not a summary
  t:expect(r.stdout):contains("promise kept")           -- the kept promise demands graduation
  t:expect(r.stdout):never():contains("arithmetic holds")  -- unflagged tests are not selected
end)

prova.test("selecting only promised nodes is a matched selection, not an empty one", {
  proves = "field-reported: `prova --node <a promised test>` ran the node, printed PROMISED, and \
then exited 2 claiming the selection matched nothing — an open promise is a selected, executed \
node whose redness is the mechanism, so it counts as matched",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. ' --node "the widget frobnicates"',
    { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("PROMISED")
  t:expect(r.stdout):never():contains("matched no tests")
end)

prova.test("the binary teaches the grammar: `prova learn promises` names the spellings", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " learn promises", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("prova tests --promises")
  t:expect(r.stdout):contains("prova tests burndown")
end)

prova.test("the retired state-verbs tombstone toward their lane spelling, never dispatch", {
  proves = "a state is a --flag on its lane now, not its own verb; muscle memory (agents most of \
all) meets a redirect, not the run path's cryptic 'no such file' — and never the old behavior",
}, function(t)
  local proj = t:use(sandbox)
  for _, pair in ipairs({
    { "promises", "prova tests --promises" },
    { "burndown", "prova tests burndown" },
    { "falsify", "prova tests falsify" },
  }) do
    local r = shell.run(prova.bin .. " " .. pair[1], { cwd = proj, merge_stderr = true })
    t:expect(r.code, pair[1] .. " no longer dispatches"):equals(2)
    t:expect(r.stdout, pair[1] .. " names its successor"):contains(pair[2])
  end
end)
