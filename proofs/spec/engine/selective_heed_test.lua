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
