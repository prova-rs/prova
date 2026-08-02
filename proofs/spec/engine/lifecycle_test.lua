--- The obligation lifecycle, as contracts rather than prose.
---
--- docs/design/lifecycle.md maps what prova's atoms turned out to add up to: an obligation has an
--- origin, it travels through stages, and each stage exists because a sentence an agent could say
--- was unfalsifiable without it. Prose that states a contract and is bound to nothing is exactly
--- the drift the claims subsystem exists to catch, so the doc's normative statements are anchored
--- and discharged here.
---
--- Two of its claims are deliberately NOT covered — `ledger-is-the-account` and
--- `ci-can-ask-for-everything` describe a verb that does not exist yet. They report UNPROVEN in
--- `prova owed`, which is the ledger doing its job on its own design doc.

local sandbox = prova.fixture("lifecycle-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  -- Level 0 and nothing else: no [claims], no spec flag, no falsifier.
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/plain_test.lua", [[
prova.test("arithmetic holds", function(t)
  t:expect(2 + 2):equals(4)
end)
]])
  return proj
end)

prova.test("no stage of the lifecycle requires an adjacent one", {
  covers = "docs/design/lifecycle.md#lifecycle-stages",
  proves = "the stages are a map, not a ladder. A project that wants claims without spec-first, or falsifiers without either, must not be told to adopt machinery it did not ask for — that is how a lifecycle becomes ceremony and gets abandoned",
}, function(t)
  local proj = t:use(sandbox)
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[claims]\ndocs = ["docs"]\n')
  fs.write(proj .. "/docs/d.md", "# D\n\n<!-- claim: solo -->\nA claim with no spec.\n")

  -- Each attribute alone, and one carrying two: every combination is a legal declaration.
  fs.write(proj .. "/proofs/combos_test.lua", [[
prova.test("covers with no spec", { covers = "docs/d.md#solo" }, function(t)
  t:expect(1):equals(1)
end)

prova.test("promises with no covers", { promises = "not built" }, function(t)
  t:expect(1):equals(2)
end)

prova.test("falsifier with neither", {
  falsified_by = function(t) error("mutated") end,
}, function(t)
  t:expect(1):equals(1)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/combos_test.lua")

  -- The open promise reports as one and does not sink the run; the other two are ordinary passes.
  t:expect(r.code, "an open promise keeps CI green"):equals(0)
  t:expect(r.stdout):contains("PROMISED")
end)

prova.test("a package at level 0 pays nothing for the levels it did not adopt", {
  covers = "docs/design/lifecycle.md#levels-are-independent",
  proves = "the load-bearing promise of the whole design. Every atom is opt-in, and machinery that taxes a project which never asked for it is machinery that gets switched off — at which point it protects nobody",
}, function(t)
  local proj = t:use(sandbox)

  -- No [claims], no spec, no falsifier: an ordinary run, and the atoms are wholly absent from it.
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 passed")
  t:expect(r.stdout, "no lecture about claims"):never():contains("owed")
  t:expect(r.stdout, "no lecture about specs"):never():contains("spec")
  t:expect(r.stdout, "no lecture about falsifiers"):never():contains("vacuous")
end)

prova.test("a query verb never executes a proof body", {
  covers = "docs/design/lifecycle.md#two-verb-families",
  proves = "reading what a package owes must be safe on any machine, whatever the proofs would do if run. A ledger that provisions containers or drives a display to answer `what is owed` is one nobody dares run in CI, and the reconciliation is a static question anyway",
}, function(t)
  local proj = t:use(sandbox)
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[claims]\ndocs = ["docs"]\n')
  fs.write(proj .. "/docs/d.md", "# D\n\n<!-- claim: watched -->\nA claim.\n")

  -- The body writes a file. If a query verb executes it, the file appears — an observable that
  -- cannot be faked by output parsing.
  local witness = proj .. "/BODY_RAN"
  fs.write(proj .. "/proofs/effect_test.lua", ([[
prova.test("has a side effect", { covers = "docs/d.md#watched" }, function(t)
  fs.write(%q, "the body executed")
  t:expect(1):equals(1)
end)
]]):format(witness))

  for _, verb in ipairs({ "owed", "specs", "attest docs/d.md#watched" }) do
    fs.remove_all(witness)
    shell.run(prova.bin .. " " .. verb, { cwd = proj, merge_stderr = true })
    t:expect(fs.exists(witness), verb .. " must not run the body"):equals(false)
  end

  -- And the control: a real run DOES execute it, so the assertion above is not vacuous.
  fs.remove_all(witness)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(fs.exists(witness), "a run executes the body"):equals(true)
  fs.remove_all(proj .. "/proofs/effect_test.lua")
end)

prova.test("prova declares claims over its own design docs and owes against them", {
  covers = "docs/design/lifecycle.md#prova-dogfoods-its-own-lifecycle",
  proves = "the exemplar argument, held as a regression guard. Deleting prova's own [claims] section would silently retire every anchor in its design docs, and the first sign would be a ledger that had quietly gone empty",
}, function(t)
  -- prova.root is this repository: the subject here is prova's own manifest, deliberately.
  local manifest = fs.read(prova.root .. "/.prova.toml")
  t:expect(manifest, "the opt-in is declared"):contains("[claims]")
  t:expect(manifest):contains("docs/design")

  -- And the anchors it points at are real, in the doc that states the lifecycle.
  local doc = fs.read(prova.root .. "/docs/design/lifecycle.md")
  t:expect(doc, "anchored, not merely asserted"):contains("<!-- claim: lifecycle-stages -->")
end)
