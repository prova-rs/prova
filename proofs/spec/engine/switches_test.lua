--- Switches — opt-in test classes, fail-closed at the declaration site
--- (docs/design/manifest.md#switches-not-env-capabilities), proven black-box through sandbox
--- packages driven by prova.bin.
---
--- `switch = "<class>"` on a test (or a group, or `suite.config`) means OFF UNLESS THROWN: the
--- bare run holds the class back — deselected, never skipped — and reports it as one summary
--- line. The doors that throw: the CLI's `-s`, and `switches = [...]` on `[run]` or a profile
--- (union across all doors). Intent lives on the selection axis; `requires` keeps the world
--- facts. Exact `--node` naming a switched test implies the throw; fuzzy selectors never do.

local function mkpkg(root, manifest, proof)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", manifest)
  fs.write(proj .. "/proofs/mixed_test.lua", proof)
  return proj
end

local MANIFEST = '[run]\nproofs = ["proofs"]\n'

-- Two ordinary tests and one behind the `heavy` switch — the class made observable.
local MIXED = [[
prova.test("ordinary one", function(t) t:expect(1):equals(1) end)
prova.test("ordinary two", function(t) t:expect(2):equals(2) end)

prova.test("conducts the expensive thing", { switch = "heavy" }, function(t)
  fs.write(prova.root .. "/heavy.ran", "yes")
  t:expect(true):is_true()
end)
]]

local scratch = prova.fixture("switches-scratch", Scope.Test, function(ctx)
  return ctx:tempdir()
end)

prova.test("a switched test is off unless thrown, and the bare run says so in one line", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "fail-closed at the declaration site: no env var, no capability registration, no manifest exclusion to forget — and one summary line teaches the class exists",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, MIXED)
  local bare = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(bare.code, bare.stdout):equals(0)
  t:expect(bare.stdout):contains("2 passed")
  t:expect(fs.exists(proj .. "/heavy.ran"), "the class must not fire unasked"):is_false()
  -- Deselected, never skipped: held back as a selection fact, not a world gap.
  t:expect(bare.stdout):never():contains("SKIP")
  t:expect(bare.stdout):contains("switched off: heavy (1)")
  t:expect(bare.stdout):contains("-s <switch>")
  -- Thrown with -s: the class joins the ordinary membership, and the line is gone.
  local thrown = shell.run(prova.bin .. " -s heavy", { cwd = proj, merge_stderr = true })
  t:expect(thrown.code, thrown.stdout):equals(0)
  t:expect(thrown.stdout):contains("3 passed")
  t:expect(fs.exists(proj .. "/heavy.ran")):is_true()
  t:expect(thrown.stdout):never():contains("switched off")
end)

prova.test("profiles and [run] throw switches; the doors union", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "`prova run full` is baked `-s` — the contract door and the courtesy door reach the same authorization bit, so there is one mechanism, not two",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST .. '\n[profiles.full]\nswitches = ["heavy"]\n', MIXED)
  local via_profile = shell.run(prova.bin .. " run full", { cwd = proj, merge_stderr = true })
  t:expect(via_profile.code, via_profile.stdout):equals(0)
  t:expect(via_profile.stdout, "the profile's switches throw the class"):contains("3 passed")
  -- [run]-level: the package bakes the throw, so the BARE run includes the class.
  local baked = mkpkg(t:use(scratch) .. "/baked", '[run]\nproofs = ["proofs"]\nswitches = ["heavy"]\n', MIXED)
  local bare = shell.run(prova.bin, { cwd = baked, merge_stderr = true })
  t:expect(bare.stdout, "[run] switches is the always-on door"):contains("3 passed")
end)

prova.test("exact --node implies the throw; a fuzzy selector never does", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "deselecting a test the caller named precisely is the swallowed-selector dishonesty; a keyword grazing a switched test conducting a workspace compile is the opposite failure",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, MIXED)
  -- Exact address = maximal intent: the switch is implied thrown for that leaf.
  local node = shell.run(prova.bin .. ' --node "conducts the expensive thing"', { cwd = proj, merge_stderr = true })
  t:expect(node.code, node.stdout):equals(0)
  t:expect(node.stdout):contains("1 passed")
  t:expect(fs.exists(proj .. "/heavy.ran"), "--node runs precisely what it names"):is_true()
  -- Fuzzy: the class stays held back, and a selection left empty by it is LOUD, not green.
  fs.remove_all(proj .. "/heavy.ran")
  local fuzzy = shell.run(prova.bin .. " -k expensive", { cwd = proj, merge_stderr = true })
  t:expect(fuzzy.code, "an unthrown class cannot be reached by keyword"):equals(2)
  t:expect(fs.exists(proj .. "/heavy.ran")):is_false()
  -- Thrown AND selected composes: authorization is not selection.
  local both = shell.run(prova.bin .. " -s heavy -k expensive", { cwd = proj, merge_stderr = true })
  t:expect(both.code, both.stdout):equals(0)
  t:expect(both.stdout):contains("1 passed")
end)

prova.test("a scope's switch gates everything under it: group-level and suite-level", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "test-by-test application would be tedious — a group or suite marked once puts its whole subtree behind the class",
}, function(t)
  local root = t:use(scratch)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", MANIFEST)
  fs.write(proj .. "/proofs/grouped_test.lua", [[
prova.test("always on", function(t) t:expect(true):is_true() end)

prova.group("the soak battery", { switch = "soak" }, function(g)
  g:test("slow one", function(t) t:expect(true):is_true() end)
  g:test("slow two", function(t) t:expect(true):is_true() end)
end)
]])
  fs.mkdir(proj .. "/proofs/gated")
  fs.write(proj .. "/proofs/gated/suite.lua", 'suite.config { switch = "ut" }\n')
  fs.write(proj .. "/proofs/gated/adopt_test.lua", [[
prova.test("adopts the deputy", function(t) t:expect(true):is_true() end)
]])
  local bare = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(bare.code, bare.stdout):equals(0)
  t:expect(bare.stdout):contains("1 passed")
  t:expect(bare.stdout, "both scopes report, per class"):contains("switched off: soak (2), ut (1)")
  local soak = shell.run(prova.bin .. " -s soak", { cwd = proj, merge_stderr = true })
  t:expect(soak.stdout, "the group's class throws as one"):contains("3 passed")
  t:expect(soak.stdout):contains("switched off: ut (1)")
  local all = shell.run(prova.bin .. " -s soak,ut", { cwd = proj, merge_stderr = true })
  t:expect(all.stdout, "-s takes a comma list, like --tags"):contains("4 passed")
end)

prova.test("`prova switches` is the ledger: every class, its size, and who throws it", {
  covers = "docs/design/manifest.md#switches-are-discoverable",
  proves = "a switched class no profile throws must be a stated fact (`ad-hoc only`), not an accident discovered when someone asks why a gate never ran",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST .. '\n[profiles.full]\nswitches = ["heavy"]\n', MIXED .. [[
prova.test("orphan gate", { switch = "bench" }, function(t) t:expect(true):is_true() end)
]])
  local r = shell.run(prova.bin .. " switches", { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("heavy")
  t:expect(r.stdout):contains("1 gated")
  t:expect(r.stdout):contains("profile `full`")
  -- The footgun row: a class nobody throws says so, in words.
  t:expect(r.stdout):contains("bench")
  t:expect(r.stdout):contains("ad-hoc only")
  -- And a package with no switches teaches the primitive instead of listing nothing silently.
  local plain = mkpkg(t:use(scratch) .. "/plain", MANIFEST, [[
prova.test("ordinary", function(t) t:expect(true):is_true() end)
]])
  local none = shell.run(prova.bin .. " switches", { cwd = plain, merge_stderr = true })
  t:expect(none.code):equals(0)
  t:expect(none.stdout):contains("no switches declared")
end)
