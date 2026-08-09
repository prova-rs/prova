--- Falsifiers — proving that a proof can fail.
---
--- A proof that has only ever been green is not evidence. It might be checking the contract, or it
--- might be checking nothing: an assertion over a value that cannot vary, a rule whose subject the
--- implementation quietly refuses in every case, a bar satisfied by a stub. Those read exactly like
--- a working proof — same colour, same duration, same line in the report — and the difference only
--- surfaces when something breaks in production that the suite swore was covered.
---
--- `falsified_by` closes that gap by making the negative case declarable instead of remembered:
--- a mutation that MUST turn the body red. `prova tests falsify` applies it and inverts the verdict,
--- so a body that survives its own falsifier is reported as vacuous.
---
--- This is the atom that would have caught two real holes: a version-constraint proof satisfied by
--- a broker that refuses every constraint outright, and an accessibility bar that passed with a
--- deliberately unlabelled control on screen. Both were found by hand, and only because someone
--- thought to look.

local sandbox = prova.fixture("falsify-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  -- Three shapes: a proof whose falsifier bites, one whose falsifier does NOT (the vacuous case
  -- this feature exists to expose), and one that declares no falsifier at all.
  fs.write(proj .. "/proofs/widget_test.lua", [[
local state = { broken = false }

prova.test("the widget reports its health", {
  falsified_by = function(t) state.broken = true end,
}, function(t)
  t:expect(state.broken):equals(false)
end)

prova.test("two plus two", {
  falsified_by = function(t) state.broken = true end,
}, function(t)
  -- Nothing the falsifier touches can make this false. That is exactly the shape of a proof
  -- that looks like evidence and is not.
  t:expect(2 + 2):equals(4)
end)

prova.test("carries no falsifier", function(t)
  t:expect(1):equals(1)
end)
]])
  return proj
end)

prova.test("a falsifier that bites proves the assertion is load-bearing", {
  proves = "the inversion is the whole mechanism: under falsification a red body is the PASSING result, because what is being proven is the body's capacity to fail",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " tests falsify", { cwd = proj, merge_stderr = true })

  -- Red under mutation is the PASSING result here: the verdict is inverted, because what is being
  -- proven is the body's capacity to fail.
  t:expect(r.stdout, "the biting falsifier is satisfied"):contains("reports its health")
  t:expect(r.stdout):never():contains("reports its health — vacuous")
end)

prova.test("a body that survives its falsifier is reported as vacuous", {
  proves = "the reason this exists. A proof that cannot fail still reports green forever, and is indistinguishable from one that works until something it swore was covered breaks",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " tests falsify", { cwd = proj, merge_stderr = true })

  -- The whole point. `2 + 2 == 4` is true no matter what the falsifier does, so the proof asserts
  -- nothing about the system and must say so out loud rather than adding to the green count.
  t:expect(r.code, "a vacuous proof fails the run"):never():equals(0)
  t:expect(r.stdout, "and names why"):contains("vacuous")
  t:expect(r.stdout):contains("two plus two")
end)

prova.test("falsify selects only what declares a falsifier", {
  proves = "the verb IS the selection, as with burndown. Most proofs will never declare a mutation, and treating their absence as failure would make the pass unusable",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " tests falsify", { cwd = proj, merge_stderr = true })

  -- Mirrors `burndown`: the verb IS the selection. A proof with no falsifier is not a failure —
  -- most proofs will never declare one — it is simply not what this pass is about.
  t:expect(r.stdout):never():contains("carries no falsifier")
end)

prova.test("a normal run is unaffected by a declared falsifier", {
  proves = "if a bare `prova` started perturbing systems, nobody would ever declare a falsifier — the cost has to sit behind the verb that asks for it",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })

  -- Declaring a falsifier must cost nothing on the ordinary path: the mutation runs only under the
  -- verb that asks for it. If `prova` alone started perturbing systems, nobody would declare one.
  t:expect(r.code, "the suite is green as written"):equals(0)
  t:expect(r.stdout):contains("3 passed")
end)

prova.test("the binary teaches the verb, catalog and topic alike", {
  proves = "a capability an agent cannot discover does not exist, and discovery is two steps: the catalog has to name it, then its topic has to explain it. Checking only one leaves a capability that is either unfindable or unexplained",
}, function(t)
  local proj = t:use(sandbox)

  -- Step one: an agent scanning the catalog must SEE it exists.
  local catalog = shell.run(prova.bin .. " learn", { cwd = proj, merge_stderr = true })
  t:expect(catalog.code):equals(0)
  t:expect(catalog.stdout, "the catalog names the topic"):contains("falsify")

  -- Step two: the topic must actually teach the verb and the attribute.
  local topic = shell.run(prova.bin .. " learn falsify", { cwd = proj, merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the driver"):contains("prova tests falsify")
  t:expect(topic.stdout, "the attribute"):contains("falsified_by")

  -- And the spec lifecycle points at it, so an agent reading about specs finds the next step.
  local specs = shell.run(prova.bin .. " learn promises", { cwd = proj, merge_stderr = true })
  t:expect(specs.stdout, "promises points onward"):contains("learn falsify")
end)

prova.test("a falsifier must be a function", {
  proves = "a silently-ignored falsifier is worse than none — the suite would claim a rigor it does not have",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/bad_test.lua", [[
prova.test("misdeclared", { falsified_by = "make it fail" }, function(t)
  t:expect(1):equals(1)
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/bad_test.lua")

  -- Rejected at declaration with the fix, in the house style: a wrong shape is a typo caught now,
  -- not a mutation that silently never runs.
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("falsified_by")
end)
