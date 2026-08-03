--- Black-box surface of manifest discovery — where the manifest may live, which one wins, and
--- what the "home" means for everything that follows from it.
---
--- The contract (docs/design/manifest.md): four layouts, one home — the project root in every
--- case. Every manifest-relative path and generated artifact resolves against the home, never
--- against the manifest's own directory. Discovery walks up from wherever prova was invoked and
--- the nearest manifest wins. Two variants in one directory is an error, and a nested manifest
--- deeper in the tree is another package entirely.

--- A fresh empty directory per call; each test builds its own layout in one. All of them are
--- torn down with the file scope.
local scratch = prova.fixture("discovery-scratch", Scope.File, function(ctx)
  return function() return ctx:tempdir() end
end)

--- Run `prova` in `dir`. Diagnostics land on stderr, so it is folded in. PROVA_VAR_DIR is
--- neutralized (empty = unset) so the state-location assertions hold even when the outer suite
--- itself was invoked with an overridden state root.
local function run(dir, args)
  return shell.run(prova.bin .. (args and (" " .. args) or ""), {
    cwd = dir,
    env = { PROVA_VAR_DIR = "" },
    merge_stderr = true,
  })
end

-- ── the home is the root, and every path resolves against it ─────────────────────────────────

prova.test("a nook manifest's paths resolve against the home, not the manifest's own directory",
  { covers = "docs/design/manifest.md#paths-resolve-against-home" }, function(t)
  -- The visible nook: prova/prova.toml, home = the directory ABOVE prova/. Both directions of
  -- the claim are pinned — `proofs` is found relative to the home, and the generated state dir
  -- lands at <home>/.prova/var, not beside the manifest. The run fails deliberately so state is
  -- actually written (the record is the first state write; a green run has nothing to prove here).
  local root = t:use(scratch)()
  fs.mkdir(root .. "/prova")
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the nook proof fails", function(t) t:expect(1):equals(2) end)\n')

  local r = run(root)
  t:expect(r.stdout, "the home-relative proof dir was discovered"):contains("the nook proof fails")

  t:expect(root .. "/.prova/var/last-failed.json"):exists()
  t:expect(fs.exists(root .. "/prova/.prova"), "nothing beside the manifest"):equals(false)
end)

-- ── discovery walks up, and the nearest manifest wins ────────────────────────────────────────

prova.test("prova runs from anywhere inside the package — including from inside the nook",
  { covers = "docs/design/manifest.md#nearest-manifest-wins" }, function(t)
  -- The hidden nook this time: .prova/prova.toml. Invoked from the home, from the proof dir,
  -- and from inside .prova/ itself — the last is the bare-manifest-in-a-dir-named-.prova case,
  -- which must root at the PARENT rather than treat .prova/ as a flat package of its own.
  local root = t:use(scratch)()
  fs.mkdir(root .. "/.prova")
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/.prova/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the proof runs", function(t) t:expect(1):equals(1) end)\n')

  for _, from in ipairs({ root, root .. "/proofs", root .. "/.prova" }) do
    local r = run(from)
    t:expect(r.code, "green from " .. from):equals(0)
    t:expect(r.stdout, "same package from " .. from):contains("the proof runs")
  end
end)

-- ── two variants in one directory is an error ────────────────────────────────────────────────

prova.test("two manifest variants in one directory are refused, naming both",
  { covers = "docs/design/manifest.md#one-manifest-per-directory" }, function(t)
  -- Picking either file silently would make the run's meaning depend on an ordering the user
  -- never chose. The refusal names both candidates and says what to do.
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("never reached", function(t) t:expect(1):equals(1) end)\n')

  local r = run(root)
  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout):contains("ambiguous")
  t:expect(r.stdout, "names the visible variant"):contains("prova.toml")
  t:expect(r.stdout, "names the hidden variant"):contains(".prova.toml")
  t:expect(r.stdout, "says what to do"):contains("keep exactly one")
  t:expect(r.stdout, "no proof was attempted"):never():contains("never reached")
end)

-- ── a nested manifest is another package ─────────────────────────────────────────────────────

prova.test("a parent run never crosses into a nested package, and the child stands alone",
  { covers = "docs/design/manifest.md#nested-package-isolation" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/child/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/o_test.lua",
    'prova.test("outer runs", function(t) t:expect(1):equals(1) end)\n')
  fs.write(root .. "/child/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/child/proofs/i_test.lua",
    'prova.test("inner runs", function(t) t:expect(1):equals(1) end)\n')

  -- The parent's discovery prunes the child — its proofs are not the parent's to run.
  local outer = run(root)
  t:expect(outer.code):equals(0)
  t:expect(outer.stdout):contains("outer runs")
  t:expect(outer.stdout, "the child's proofs are not the parent's"):never():contains("inner runs")

  -- From inside the child, the child's manifest is the nearest — the walk stops there.
  local inner = run(root .. "/child")
  t:expect(inner.code):equals(0)
  t:expect(inner.stdout):contains("inner runs")
  t:expect(inner.stdout, "the parent's proofs are not the child's"):never():contains("outer runs")
end)
