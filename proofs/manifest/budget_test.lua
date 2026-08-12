--- Lane time budgets (docs/design/manifest.md#lane-time-budgets): a lane states the wall time
--- its composition promises, and the run goes red past it — even all-green. The field report:
--- five graduated proofs landed switchless in a default lane and the seconds-fast inner loop
--- silently became a 10-minute gate, caught only by a human noticing. Budgets never inherit:
--- `[run]`'s binds only the bare run, a profile's only that profile — the heavy lanes are
--- exactly the ones a bare-run budget must not leak onto.

--- A package whose one test sleeps ~50ms, under the given manifest — enough duration that a
--- "1ms" budget always exceeds and a "60s" budget never does, deterministic on any host.
local function package(t, manifest)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", manifest)
  fs.write(proj .. "/proofs/paced_test.lua", [[
prova.test("green but not instant", function(t)
  shell.run("sleep 0.05")
  t:expect(true):is_true()
end)
]])
  return proj
end

prova.test("an all-green run past its budget is red, naming the overage and the cure", {
  covers = "docs/design/manifest.md#lane-time-budgets",
  proves = "the regression arrives green — nothing fails when a conduct lands in the fast lane, the lane just quietly stops being fast — so the budget must gate on time alone, and the message must teach where heavy proofs belong (switches), not merely scold",
}, function(t)
  local proj = package(t, '[run]\nproofs = ["proofs"]\nbudget = "1ms"\n')
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })

  t:expect(r.stdout, "the test itself is green"):contains("1 passed")
  t:expect(r.code, "the run is red anyway"):never():equals(0)
  t:expect(r.stdout, "the failure names the bar"):contains("over budget")
  t:expect(r.stdout, "…and teaches the cure"):contains("switches")
end)

prova.test("a run within its budget stays green", {
  covers = "docs/design/manifest.md#lane-time-budgets",
  proves = "the gate must price the composition, not tax it — a budget generous enough for the lane changes nothing about a green run",
}, function(t)
  local proj = package(t, '[run]\nproofs = ["proofs"]\nbudget = "60s"\n')
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):never():contains("over budget")
end)

prova.test("a budget never inherits: [run]'s binds the bare run, a profile's only itself", {
  covers = "docs/design/manifest.md#lane-time-budgets",
  proves = "inheritance would leak the fast lane's bar onto the heavy lanes — `run all` legitimately takes what the bare run must never take, so the sweep profile without a budget must not inherit the 1ms one that just failed the bare run",
}, function(t)
  local proj = package(t,
    '[run]\nproofs = ["proofs"]\nbudget = "1ms"\n\n[profiles.sweep]\ndescription = "the heavy leg"\n')
  local bare = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(bare.code, "the bare run holds [run]'s budget"):never():equals(0)

  local sweep = shell.run(prova.bin .. " run sweep", { cwd = proj, merge_stderr = true })
  t:expect(sweep.code, "the profile inherits no budget"):equals(0)
  t:expect(sweep.stdout):never():contains("over budget")

  -- And the mirror: a profile's own budget binds it while the bare run stays unpriced.
  local proj2 = package(t,
    '[run]\nproofs = ["proofs"]\n\n[profiles.fast]\nbudget = "1ms"\n')
  local bare2 = shell.run(prova.bin, { cwd = proj2, merge_stderr = true })
  t:expect(bare2.code, "no [run] budget, no bare gate"):equals(0)
  local fast = shell.run(prova.bin .. " run fast", { cwd = proj2, merge_stderr = true })
  t:expect(fast.code, "the profile's own budget binds"):never():equals(0)
  t:expect(fast.stdout):contains("over budget")
end)

prova.test("the lane's budget is a visible fact: run --list chips it, a bad value refuses", {
  covers = "docs/design/manifest.md#lane-time-budgets",
  proves = "a bar nobody can see is a bar nobody placed deliberately — the lane listing must show it; and a typo'd duration must refuse at resolve, never parse to 'no budget' and silently unguard the lane",
}, function(t)
  local proj = package(t,
    '[run]\nproofs = ["proofs"]\n\n[profiles.fast]\nbudget = "30s"\n')
  local listing = shell.run(prova.bin .. " run --list", { cwd = proj, merge_stderr = true })
  t:expect(listing.stdout, "the chip names the bar"):contains("budget: 30s")

  local bad = package(t, '[run]\nproofs = ["proofs"]\nbudget = "eventually"\n')
  local r = shell.run(prova.bin, { cwd = bad, merge_stderr = true })
  t:expect(r.code, "an unparseable budget refuses"):never():equals(0)
  t:expect(r.stdout, "…naming the value"):contains("eventually")
end)
