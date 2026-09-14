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

-- ── bounding the mutant (docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant) ──────────────
--
-- The mutation class falsify most exists to catch — delete a terminating condition — is exactly
-- the class that produces no verdict at all. So `falsify` has to assume the mutant is hostile to
-- termination, and these proofs are about the verb staying SOUND under that assumption rather than
-- about a convenience flag.

local hanging = prova.fixture("hanging-mutant-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir("hang") .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  -- A POLLING watcher, which is the witness's shape: it awaits every round, so a deadline can
  -- reach it. The falsifier removes the end condition, exactly as removing a grace check did in
  -- Substrate's slot watcher — the unmutated body terminates immediately, the mutated one never
  -- does. Nothing here declares a `timeout`, because the whole point is that nobody does.
  fs.write(proj .. "/proofs/watcher_test.lua", [[
local state = { settled = true }

prova.test("the watcher stops when the slot settles", {
  falsified_by = function(t) state.settled = false end,
}, function(t)
  local rounds = 0
  while not state.settled do
    rounds = rounds + 1
    prova.sleep(20)
  end
  t:expect(rounds):equals(0)
end)
]])
  return proj
end)

prova.test("a mutant that never returns is bounded and reported as VACUOUS-HANG", {
  covers = "docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant",
  proves = "the soundness case. `invert_for_falsify` is total over {Passed, Failed, Skipped}, but running a body can also produce no outcome at all — and ⊥ has no inverse. Before this the run simply never came back, which is the one failure an agent cannot distinguish from slow work",
}, function(t)
  local proj = t:use(hanging)
  -- The bound must come from the RUN, not the proof: this sandbox declares no timeout anywhere,
  -- because an author who remembered to declare one would not have hit the bug.
  local r = shell.run({ prova.bin, "tests", "falsify", "--timeout", "3s" },
    { cwd = proj, merge_stderr = true, timeout = "90s" })

  t:expect(r.stdout, "the hang is named as its own verdict, not as a generic timeout"):contains("VACUOUS-HANG")
  t:expect(r.code, "and it fails the run"):never():equals(0)
end)

prova.test("a hang is never inverted into a falsification success", {
  covers = "docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant",
  proves = "the trap this pins shut. Under falsify a red body inverts to green, so folding the timeout into the ordinary result path would turn every hang into the strongest possible green — one that means nothing. The non-inversion was incidental (an early return) and nothing covered it",
}, function(t)
  local proj = t:use(hanging)
  local r = shell.run({ prova.bin, "tests", "falsify", "--timeout", "3s" },
    { cwd = proj, merge_stderr = true, timeout = "90s" })

  -- Every assertion here has to be reachable ONLY through the real verdict. Asserting
  -- "not exit 0" alone was satisfied by `--timeout` being an unknown flag — a usage error is
  -- also non-zero — so this proof passed before a line of it was implemented. That is the exact
  -- absence-shaped vacuity `falsified_by` exists to expose, met while writing a falsify proof.
  t:expect(r.stdout, "the run reached the VERDICT, not a usage error:\n" .. r.stdout)
    :contains("VACUOUS-HANG")
  t:expect(r.stdout, "the inversion did not fire — nothing passed"):contains("0 passed")
  t:expect(r.code, "a hung mutant is never a passing falsification"):never():equals(0)
end)

prova.test("--timeout caps every test in the run, overriding what each declares", {
  covers = "docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant",
  proves = "the affordance the witness reached for and did not find. A per-test `timeout` is the ordinary spelling and stays so; this is the debug-run override, which has to beat a declared bound or it cannot rescue a run whose declared bounds are the problem",
}, function(t)
  local proj = t:use(sandbox)
  -- A declared timeout far larger than the cap: the cap must win, or the override is decorative.
  fs.write(proj .. "/proofs/slow_test.lua", [[
prova.test("declares a generous bound of its own", { timeout = "600s" }, function(t)
  prova.sleep(30000)
end)
]])
  local r = shell.run({ prova.bin, "--timeout", "2s", "-k", "generous" },
    { cwd = proj, merge_stderr = true, timeout = "90s" })
  fs.remove_all(proj .. "/proofs/slow_test.lua")

  t:expect(r.code, "the run-scoped cap beats the declared one"):never():equals(0)
  t:expect(r.stdout, "and says which bound applied"):contains("2s")
end)

prova.test("idle_timeout is the liveness bound, and spells what shell.run spells", {
  covers = "docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant",
  proves = "one concept, not two: an author who knows shell.run's idle_timeout already knows this one. It is the bound that separates a WEDGED body from a slow one — which is what lets it be generous without being useless, where a wall-clock cap has to be wrong for somebody",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/idle_test.lua", [[
prova.test("emits nothing and asserts nothing, forever", { idle_timeout = "2s" }, function(t)
  prova.sleep(30000)
end)
]])
  local r = shell.run({ prova.bin, "-k", "emits nothing" },
    { cwd = proj, merge_stderr = true, timeout = "90s" })
  fs.remove_all(proj .. "/proofs/idle_test.lua")

  -- `contains("idle")` alone was vacuous: the closed-option refusal for an UNKNOWN `idle_timeout`
  -- also contains the word, so this passed before the option existed. Rule that path out by name.
  t:expect(r.stdout, "the option is honored, not refused:\n" .. r.stdout)
    :never():contains("unknown option")
  t:expect(r.stdout, "the TEST ran and failed (a collect-time refusal names no test)")
    :contains("emits nothing and asserts nothing")
  t:expect(r.stdout, "and the message names the liveness bound, not a wall clock")
    :contains("no progress")
  t:expect(r.code, "a body that shows no sign of life is bounded"):never():equals(0)
end)

prova.test("a test that keeps asserting is never killed by the liveness bound", {
  covers = "docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant",
  proves = "the negative control, and the property that makes a liveness bound safe to default on under falsify: `bounds death, never work`. A bound that killed a slow-but-working body would be a worse defect than the hang it replaced, because it would fail proofs that are CORRECT",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/proofs/alive_test.lua", [[
-- Longer than the idle bound overall, but never quiet for it: exactly the slow-honest-work case.
prova.test("slow, but visibly progressing", { idle_timeout = "2s" }, function(t)
  for _ = 1, 10 do
    prova.sleep(500)
    t:expect(1):equals(1)
  end
end)
]])
  local r = shell.run({ prova.bin, "-k", "visibly progressing" },
    { cwd = proj, merge_stderr = true, timeout = "90s" })
  fs.remove_all(proj .. "/proofs/alive_test.lua")

  t:expect(r.code, "progress is life: the run is green:\n" .. r.stdout):equals(0)
end)

prova.test("falsify bounds a mutant with no flags and no declared timeout at all", {
  covers = "docs/design/lifecycle.md#falsify-bounds-a-hanging-mutant",
  proves = "the witness case exactly as it was hit: no `--timeout`, no declared `timeout`, no `idle_timeout` — because an author who had declared any of them would never have found the bug. Every other proof here hands the run a bound; this one proves the run brings its own, which is the difference between a flag and a soundness fix",
  -- Waits out the real default, so it costs what the default costs. Tagged rather than trimmed:
  -- shortening the default to make its own proof cheap would be tuning the product to the test.
  tags = { "slow" },
}, function(t)
  local proj = t:use(hanging)
  -- The shell bound is the SAFETY NET, deliberately far larger than falsify's own: if the feature
  -- regresses, this fails on prova's bound rather than hanging this suite too.
  local r = shell.run({ prova.bin, "tests", "falsify" },
    { cwd = proj, merge_stderr = true, timeout = "180s" })

  t:expect(r.stdout, "the run bounded itself:\n" .. r.stdout):contains("VACUOUS-HANG")
  t:expect(r.code, "and reported a verdict rather than hanging"):never():equals(0)
end)
