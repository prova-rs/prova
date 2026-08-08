--- `promises` — the deferred-proof attribute, renamed into the grammar it always belonged to.
---
--- Every other attribute is a third-person verb: proves, covers, requires. `spec` was a noun
--- among them, named for the practice rather than for what the test itself says. `promises`
--- states the intent in the attribute's own voice — this test WILL prove that; today it does
--- not — and graduation becomes a tense change: promises → proves. The error message teaches
--- the whole lifecycle in one line.
---
--- `spec` remains as a deprecation alias for one release: it works, it warns, it names the new
--- attribute. Two permanent names for one attribute is exactly the inconsistency the rename
--- exists to end, so the alias is a bridge, not a fixture.

local sandbox = prova.fixture("promises-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  return proj
end)

prova.test("a promised proof with a red body is reported, never failed", {
  proves = "the promise IS the specification: red is its healthy state, and CI staying green over an authored-ahead backlog is what makes authoring ahead survivable",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/promise_test.lua", [[
prova.test("drain semantics hold", { promises = "needs a multi-node broker" }, function(t)
  t:expect(1):equals(2)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/promise_test.lua")

  t:expect(r.code, "an open promise keeps CI green"):equals(0)
  t:expect(r.stdout, "and is named as what it is"):contains("PROMISED")
  t:expect(r.stdout, "with its reason"):contains("needs a multi-node broker")
end)

prova.test("a kept promise fails, demanding the tense change", {
  proves = "graduation is promises → proves, a one-word edit in one grammar. The failure message carries the exact replacement, so keeping a promise and recording that it is kept land in the same commit",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/kept_test.lua", [[
prova.test("arithmetic holds", { promises = "should have been easy" }, function(t)
  t:expect(2 + 2):equals(4)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/kept_test.lua")

  t:expect(r.code, "a kept promise cannot stay flagged"):never():equals(0)
  t:expect(r.stdout):contains("promise kept")
  t:expect(r.stdout, "the fix is copy-pasteable"):contains('proves = "should have been easy"')
end)

prova.test("promises demands its reason, and refuses booleans", {
  proves = "a bare flag says nothing to the burndown, and the reason is what graduates into the proves context — so it is forced from day one, exactly as spec's was",
}, function(t)
  local proj = t:use(sandbox)
  for _, bad in ipairs({ "promises = true", 'promises = ""', "promises = false" }) do
    fs.write(proj .. "/proofs/bad_test.lua", ([[
prova.test("misdeclared", { %s }, function(t)
  t:expect(1):equals(1)
end)
]]):format(bad))
    local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
    t:expect(r.code, bad .. " is refused"):never():equals(0)
    t:expect(r.stdout, bad .. " names the attribute"):contains("promises")
  end
  fs.remove_all(proj .. "/proofs/bad_test.lua")
end)

prova.test("promises and proves never share a test", {
  proves = "a test is either an open intent or a kept one — both at once is a contradiction, and the mutual exclusion held for spec/proves must survive the rename",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/both_test.lua", [[
prova.test("contradiction", {
  promises = "not yet", proves = "already",
}, function(t)
  t:expect(1):equals(1)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/both_test.lua")

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("not both")
end)

prova.test("the run summary tallies promises in the new vocabulary", {
  proves = "the human tally and the machine fields part ways deliberately: consoles speak the vocabulary, while JSON/TAP/JUnit field names stay frozen for every parser already reading them",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/tally_test.lua", [[
prova.test("open intent", { promises = "not yet" }, function(t)
  t:expect(1):equals(2)
end)
prova.test("plain pass", function(t)
  t:expect(1):equals(1)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/tally_test.lua")

  t:expect(r.stdout, "the tally speaks it"):contains("1 promised")
  t:expect(r.stdout):never():contains("spec open")
end)

