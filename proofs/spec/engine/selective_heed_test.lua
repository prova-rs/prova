-- Spec: selective heed — parity with test selection (docs/design/reminders.md). A DUE reminder is
-- non-fatal by default; `--heed` heeds all; `--heed=<selector>` and a profile's `heed = [...]` heed
-- only reminders matching a name or tag, so a profile shapes which attention a phase gates on.
-- Black-box via prova.bin.

-- Two reminders: "line-counts" (tag "quality") is DUE; "deps" (tag "maintenance") is WATCHING.
local PROOF = [[
  prova.remind("line-counts", { tags = { "quality" },
    when = function() return "3 files over the limit" end }, "split them")
  prova.remind("deps", { tags = { "maintenance" },
    when = function() return nil end }, "bump dependencies")
  prova.test("noop", function(t) t:expect(1):equals(1) end)
]]

local function workspace(t, extra_manifest)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n' .. (extra_manifest or ""))
  fs.write(dir .. "/proofs/remind_test.lua", PROOF)
  return dir
end

local function run(dir, ...)
  return shell.run({ prova.bin, ... }, { cwd = dir, merge_stderr = true })
end

prova.test("a DUE reminder is non-fatal by default", function(t)
  local r = run(workspace(t))
  t:expect(r.code, r.stdout):equals(0)
end)

prova.test("--heed makes a DUE reminder fatal", function(t)
  local r = run(workspace(t), "--heed")
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("heeded reminder")
end)

prova.test("--heed=<name> heeds only the matching reminder", function(t)
  local dir = workspace(t)
  t:expect(run(dir, "--heed=line-counts").code, "line-counts is DUE and heeded"):never():equals(0)
  -- selectivity: deps is heeded but WATCHING, so nothing fatal fires
  t:expect(run(dir, "--heed=deps").code, "deps is watching, not due"):equals(0)
end)

prova.test("--heed=<tag> heeds by tag", function(t)
  local r = run(workspace(t), "--heed=quality")
  t:expect(r.code):never():equals(0) -- line-counts carries tag "quality" and is DUE
end)

prova.test("a profile's heed list heeds a subset (phase shaping)", function(t)
  local dir = workspace(t, '[profiles.strict]\nheed = ["line-counts"]\n')
  t:expect(run(dir, "--profile", "strict").code):never():equals(0)
  -- a profile heeding only "deps" stays green (deps is watching)
  local dir2 = workspace(t, '[profiles.deps-only]\nheed = ["deps"]\n')
  t:expect(run(dir2, "--profile", "deps-only").code, "deps watching"):equals(0)
end)

prova.test("heed speaks the one selector grammar — `!` excludes, exactly as everywhere else", {
  covers = "docs/design/reminders.md#heed-selector-is-the-one-grammar",
  proves = "matches_selector was the last private selector dialect in the tree (substring-or-exact-tag, two lines, 'mirroring selection in spirit') — heed now constructs the same Selection the lane report narrows through, so the grammar cannot drift between reading the lane and gating on it",
}, function(t)
  -- `--heed=!maintenance` heeds everything NOT maintenance-tagged: line-counts (quality) is DUE
  -- and heeded — fatal.
  local dir = workspace(t)
  t:expect(run(dir, "--heed=!maintenance").code, "excluded-by-tag leaves quality heeded"):never():equals(0)

  -- Excluding the quality tag spares the one DUE reminder — the run stays green even though
  -- everything else is heeded.
  t:expect(run(dir, "--heed=!quality").code, "the DUE reminder is excluded"):equals(0)

  -- The exclude composes with an include, `-k`-style: heed line-counts but not its tag's
  -- siblings — include-then-exclude narrows, exactly as test selection does.
  t:expect(run(dir, "--heed=line-counts,!line-counts").code, "an exclude beats its include"):equals(0)
end)
