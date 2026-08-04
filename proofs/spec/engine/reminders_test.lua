--- Reminders — the attention account (docs/design/reminders.md), proven black-box through
--- sandbox child packages driven by prova.bin.
---
--- A reminder is an obligation the WORLD creates: `prova.remind(name, { when = fn }, message)`
--- declares a condition and an instruction; the condition evaluates during runs, after the
--- proofs, and the query verbs read the record back. These proofs hold the design doc's claims:
--- the two-account separation, the promises firewall, non-fatal DUE, query purity, ledger
--- conditions, and the no-fixpoint rule.

--- Write a sandbox package: a manifest, and one proofs file with the given body.
local function mkpkg(root, manifest, proof)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", manifest)
  fs.write(proj .. "/proofs/watch_test.lua", proof)
  return proj
end

local MANIFEST = '[run]\nproofs = ["proofs"]\n'

-- One green test, one reminder that is DUE (with a why), one that is WATCHING. The shared package
-- for tests that only assert on their own invocation's output.
local DUE_PROOF = [[
prova.test("arithmetic holds", function(t)
  t:expect(1 + 1):equals(2)
end)

prova.remind("upstream shipped", {
  when = function() return "v9 is out" end,
}, "bump the pin")

prova.remind("nothing to see", {
  when = function() return false end,
}, "never fires")
]]

local due_pkg = prova.fixture("due-pkg", Scope.File, function(ctx)
  return mkpkg(ctx:tempdir(), MANIFEST, DUE_PROOF)
end)

-- A per-test root for packages whose RECORD is asserted on (a shared package would race:
-- sibling tests run concurrently and each full run rewrites .prova/var/).
local scratch = prova.fixture("reminders-scratch", Scope.Test, function(ctx)
  return ctx:tempdir()
end)

prova.test("the run headline is the evidence account; fired reminders add their own section", {
  covers = "docs/design/reminders.md#two-accounts",
  proves = "conflate the accounts and each destroys the other's signal: world-motion blocks merges, or regressions hide in a wall of nags",
}, function(t)
  local proj = t:use(due_pkg)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  -- The tally counts tests alone: two reminders declared, none of them in it.
  t:expect(r.stdout):contains("1 passed, 0 failed, 0 skipped")
  -- The fired reminder is its own section — name, the condition's why, the instruction.
  t:expect(r.stdout):contains("DUE  upstream shipped — v9 is out")
  t:expect(r.stdout):contains("bump the pin")
  t:expect(r.stdout):contains("1 reminder due")
  -- A watching reminder is SILENCE in the run output, not a PASS line.
  t:expect(r.stdout):never():contains("nothing to see")
end)

prova.test("a reminder is not a promise: invisible to the spec surface, never demanding graduation", {
  covers = "docs/design/reminders.md#reminders-are-not-promises",
  proves = "the promise-body-as-trigger hack made every dormant tripwire an open promise; keeping the constructs separate is what keeps `promises` pure",
}, function(t)
  local proj = t:use(due_pkg)
  local p = shell.run(prova.bin .. " promises", { cwd = proj, merge_stderr = true })
  t:expect(p.stdout):never():contains("upstream shipped")
  local l = shell.run(prova.bin .. " list", { cwd = proj, merge_stderr = true })
  t:expect(l.stdout, "a reminder is not a node"):never():contains("upstream shipped")
  -- A condition holding true is DUE — never a kept promise demanding its flag removed.
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.stdout):never():contains("promise kept")
end)

prova.test("attention is not implementation: burndown never selects reminders, --due tolerates them", {
  covers = "docs/design/reminders.md#attention-not-implementation",
  proves = "an agent handed 'upstream has not released yet' as work to implement is the mechanical collision that forced reminders to be first-class",
}, function(t)
  local root = t:use(scratch)
  local proj = mkpkg(root, MANIFEST, [[
prova.test("built later", { promises = "sandbox: open" }, function(t)
  t:expect(1):equals(2)
end)

prova.remind("world moved", {
  when = function() return true end,
}, "act on it")
]])
  -- The implementing loop selects the open promise and never the reminder.
  local b = shell.run(prova.bin .. " burndown", { cwd = proj, merge_stderr = true })
  t:expect(b.stdout):contains("built later")
  t:expect(b.stdout, "burndown must not hand a reminder to the agent"):never():contains("world moved")
  -- `--due` makes PROMISES fall due; a due REMINDER is not an open promise and fails nothing.
  local d = shell.run(prova.bin .. " --due", { cwd = t:use(due_pkg), merge_stderr = true })
  t:expect(d.code, d.stdout):equals(0)
end)

prova.test("DUE is non-fatal by default; a context that heeds fails on it", {
  covers = "docs/design/reminders.md#due-is-not-failure",
  proves = "the world moving is not a defect in the change under test — but a lane whose job is currency must be able to promise attention",
}, function(t)
  -- Default: due reminder + green proofs → exit 0 (proven on the shared package's run).
  local ok = shell.run(prova.bin, { cwd = t:use(due_pkg), merge_stderr = true })
  t:expect(ok.code, ok.stdout):equals(0)
  -- The heeding lane: same package shape, `heed_reminders = true` → the same run fails,
  -- saying why. And `--heed` promotes a single invocation identically, no manifest change.
  local proj = mkpkg(t:use(scratch), '[run]\nproofs = ["proofs"]\nheed_reminders = true\n', DUE_PROOF)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, "a heeding context fails on DUE"):never():equals(0)
  t:expect(r.stdout):contains("heed")
  local adhoc = mkpkg(t:use(scratch) .. "/adhoc", MANIFEST, DUE_PROOF)
  local a = shell.run(prova.bin .. " --heed", { cwd = adhoc, merge_stderr = true })
  t:expect(a.code, "--heed promotes one invocation"):never():equals(0)
end)

prova.test("a watcher that could not look is UNEVALUATED, never watching", {
  covers = "docs/design/reminders.md#unevaluated-never-watching",
  proves = "a disarmed tripwire that reports 'saw nothing' is the vacuous green of the attention account",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, [[
prova.test("green", function(t) t:expect(true):is_true() end)

prova.remind("needs a unicorn", {
  when = function() return true end,
  requires = { "unicorn-xyz" },
}, "act")

prova.remind("blows up", {
  when = function() error("boom") end,
}, "act")
]])
  local run = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(run.code, run.stdout):equals(0)
  local r = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "nothing is due — unevaluated does not gate"):equals(0)
  t:expect(r.stdout):contains("UNEVALUATED")
  t:expect(r.stdout):contains("unicorn")            -- the unmet capability, named
  t:expect(r.stdout):contains("condition raised")   -- the raise, named
  t:expect(r.stdout):contains("boom")
  t:expect(r.stdout):never():contains("WATCHING     needs a unicorn")
  t:expect(r.stdout):never():contains("WATCHING     blows up")
end)

-- The world-watcher package: its condition stamps a file every time it EVALUATES, and fires
-- only when the world (a flag file) has moved. The stamp is how the next two proofs observe
-- when evaluation happens — and when it must not.
local WATCHER_PROOF = [[
prova.test("green", function(t) t:expect(true):is_true() end)

prova.remind("world watcher", {
  when = function()
    fs.write(prova.root .. "/evaluated.stamp", "ran")
    return fs.exists(prova.root .. "/world.flag") and "the world moved"
  end,
}, "act on the world")
]]

prova.test("conditions evaluate during runs; the query verbs execute nothing", {
  covers = "docs/design/reminders.md#conditions-evaluate-in-runs",
  proves = "a reminder probing GitHub must not make `prova owed` a network call — the two-verb-families invariant holds for the attention account too",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, WATCHER_PROOF)
  local run = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(run.code, run.stdout):equals(0)
  t:expect(fs.exists(proj .. "/evaluated.stamp"), "the run evaluates the condition"):is_true()
  fs.remove_all(proj .. "/evaluated.stamp")
  shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })
  shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  shell.run(prova.bin .. " evidence", { cwd = proj, merge_stderr = true })
  t:expect(fs.exists(proj .. "/evaluated.stamp"), "queries read the record, never the world"):is_false()
end)

prova.test("no daemon: the recorded state changes only when a run happens", {
  covers = "docs/design/reminders.md#no-daemon",
  proves = "'whenever the world moves' means at every evaluation — prova states and reports the obligation; the scheduler is whatever already schedules runs",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, WATCHER_PROOF)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local before = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })
  t:expect(before.code):equals(0)
  t:expect(before.stdout):contains("WATCHING")
  fs.write(proj .. "/world.flag", "moved") -- the world moves...
  local still = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })
  t:expect(still.stdout, "no run, no evaluation — the record holds"):contains("WATCHING")
  t:expect(still.stdout):never():contains("the world moved")
  shell.run(prova.bin, { cwd = proj, merge_stderr = true }) -- ...and the next run sees it
  local after = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })
  t:expect(after.code):never():equals(0)
  t:expect(after.stdout):contains("DUE")
  t:expect(after.stdout):contains("world watcher — the world moved")
end)

prova.test("`prova reminders` reports every state and exits non-zero when any is due", {
  covers = "docs/design/reminders.md#reminders-verb-exit-contract",
  proves = "'is anything owed attention?' has to be one exit code, or a pipeline cannot gate on currency",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, DUE_PROOF)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "a due reminder gates the verb"):never():equals(0)
  t:expect(r.stdout):contains("DUE")
  t:expect(r.stdout):contains("upstream shipped")
  t:expect(r.stdout):contains("bump the pin")
  t:expect(r.stdout, "watching is visible HERE, unlike the run output"):contains("WATCHING")
  t:expect(r.stdout):contains("nothing to see")
  t:expect(r.stdout):contains("1 due, 0 unevaluated, 1 watching")
  -- DUE joins the one-question answer; owed still reports rather than gates.
  local o = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  t:expect(o.code):equals(0)
  t:expect(o.stdout):contains("DUE")
  t:expect(o.stdout):contains("upstream shipped")
end)

prova.test("ledger conditions: the terminal item watches while work remains, due when nothing is owed", {
  covers = "docs/design/reminders.md#ledger-conditions",
  proves = "the checklist archetype's terminal item is a reminder, not a hijacked promise — watching while work remains, DUE exactly once, when deletion is all that is left",
}, function(t)
  local TERMINAL = [[
prova.test("green", function(t) t:expect(true):is_true() end)
%s
prova.remind("this checklist has served its purpose", {
  when = function(a)
    return a.owed == 0 and a.failed == 0
       and ("all green: " .. a.passed .. " passed, " .. a.owed .. " owed")
  end,
}, "delete this directory, from outside")
]]
  -- Work remaining: an open promise keeps the ledger non-empty, so the terminal item watches.
  local open = mkpkg(t:use(scratch) .. "/open", MANIFEST, TERMINAL:format([[
prova.test("built later", { promises = "sandbox: open" }, function(t) t:expect(1):equals(2) end)
]]))
  shell.run(prova.bin, { cwd = open, merge_stderr = true })
  local watching = shell.run(prova.bin .. " reminders", { cwd = open, merge_stderr = true })
  t:expect(watching.stdout, "owed > 0 keeps the terminal watching"):contains("WATCHING")
  t:expect(watching.stdout):never():contains("DUE  this checklist")
  -- Nothing owed: the same reminder fires, and its why carries the account it observed.
  local done = mkpkg(t:use(scratch) .. "/done", MANIFEST, TERMINAL:format(""))
  shell.run(prova.bin, { cwd = done, merge_stderr = true })
  local due = shell.run(prova.bin .. " reminders", { cwd = done, merge_stderr = true })
  t:expect(due.stdout):contains("DUE")
  t:expect(due.stdout):contains("this checklist has served its purpose — all green: 1 passed, 0 owed")
  t:expect(due.stdout):contains("delete this directory")
end)

prova.test("no fixpoint: the account carries no reminder state, and evaluation is one pass in declaration order", {
  covers = "docs/design/reminders.md#no-reminder-fixpoint",
  proves = "a reminder observing reminders would make declaration order semantics and the account a fixpoint problem — evidence first, attention second, once",
}, function(t)
  local proj = mkpkg(t:use(scratch), MANIFEST, [[
prova.test("green", function(t) t:expect(true):is_true() end)

prova.remind("first", { when = function() return false end }, "n/a")

prova.remind("second", {
  when = function(a)
    -- If the account exposed reminder state in any spelling, this would stay watching.
    return a.reminders == nil and a.due == nil and a.watching == nil
       and "the account carries no reminder state"
  end,
}, "n/a")
]])
  local run = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(run.code, run.stdout):equals(0)
  t:expect(run.stdout):contains("the account carries no reminder state")
  -- The record holds the rows in declaration order — one pass, no reordering by state.
  local recorded = json.decode(fs.read(proj .. "/.prova/var/last-run.json"))
  t:expect(recorded.reminders[1].name):equals("first")
  t:expect(recorded.reminders[2].name):equals("second")
  t:expect(recorded.reminders[1].state):equals("watching")
  t:expect(recorded.reminders[2].state):equals("due")
end)

prova.test("the binary teaches the account: `prova learn reminders` names the verbs and the rule", {
  proves = "the autodidact surface is where an arriving agent learns that attention is not implementation",
}, function(t)
  local proj = t:use(due_pkg)
  local r = shell.run(prova.bin .. " learn reminders", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("prova reminders")
  t:expect(r.stdout):contains("heed")
end)
