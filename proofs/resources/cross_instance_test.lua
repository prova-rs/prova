-- The cross-instance half of `locks` (docs/design/architecture.md#locks-cross-instance): the
-- readers-writer hold is a flock in the package's var/, so a house rule like "one cargo at a
-- time" binds every prova at this home — -j 10, a second agent, CI on the same box — not just
-- the leaves of one run. Two REAL prova instances race on one writer token here; the file they
-- append to must never interleave a hold.

prova.test("a writer lock holds across two concurrent prova instances", {
  covers = "docs/design/architecture.md#locks-cross-instance",
  proves = "cargo builds take process-wide locks, so 'do not run two cargos' is a house rule a suite must be able to impose; a run-scoped table cannot — two prova instances were the loophole",
}, function(t)
  local pkg = t:tempdir()
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/hold_test.lua", [[
prova.test("hold the crunch lock", { locks = { prova.writes("crunch") } }, function(t)
  local mark = 'printf "%s %s\\n" "$RACE_TAG" "$1" >> "$RACE_LOG"'
  shell.run({ "sh", "-c", mark, "sh", "start" })
  prova.sleep(400)
  shell.run({ "sh", "-c", mark, "sh", "end" })
  t:expect(true):is_true()
end)
]])
  local log = pkg .. "/race.log"
  fs.write(log, "")

  -- Two instances, launched together, same home. The shell runs them concurrently and waits.
  local r = shell.run({
    "sh", "-c",
    'RACE_TAG=a "$0" > a.out 2>&1 & RACE_TAG=b "$0" > b.out 2>&1; A=$?; wait; [ $A -eq 0 ]',
    prova.bin,
  }, { cwd = pkg, env = { RACE_LOG = log }, timeout = "120s", merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)

  -- Both instances ran the test; the holds never interleaved: each `X start` is followed
  -- immediately by its own `X end`.
  local lines = {}
  for line in fs.read(log):gmatch("[^\n]+") do lines[#lines + 1] = line end
  t:expect(#lines, "both holds completed"):equals(4)
  t:expect(lines[1]:match("^(%a) start") ~= nil, "a hold opens first: " .. lines[1]):is_true()
  local first = lines[1]:sub(1, 1)
  t:expect(lines[2], "the first hold closes before the second opens"):equals(first .. " end")
  local second = lines[3]:sub(1, 1)
  t:expect(lines[3]):equals(second .. " start")
  t:expect(lines[4]):equals(second .. " end")
  t:expect(second ~= first, "both instances held in turn"):is_true()
end)

prova.test("`prova lock` joins the house rule from outside — exit code forwarded, hold released", {
  covers = "docs/design/architecture.md#lock-wrapper-verb",
  proves = "macOS ships no flock(1), so a Makefile or CI step had no one-line way to join a rule like 'one cargo at a time' — the wrapper is the contract's portable spelling, in the suite's own vocabulary (a bare token writes; --reads is the concurrent hold)",
}, function(t)
  local pkg = t:tempdir()
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')

  -- Two wrapped writers race: their critical sections never interleave.
  local log = pkg .. "/wrapped.log"
  fs.write(log, "")
  local mark = 'printf "%s\\n" "$1" >> ' .. log .. ' && sleep 0.3 && printf "%s\\n" "$2" >> ' .. log
  local r = shell.run({
    "sh", "-c",
    '"$0" lock crunch -- sh -c \'' .. mark .. '\' sh a-start a-end & ' ..
    '"$0" lock crunch -- sh -c \'' .. mark .. '\' sh b-start b-end; wait',
    prova.bin,
  }, { cwd = pkg, timeout = "120s", merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  local lines = {}
  for line in fs.read(log):gmatch("[^\n]+") do lines[#lines + 1] = line end
  t:expect(#lines):equals(4)
  t:expect(lines[2], "the first hold closed before the second opened")
    :equals(lines[1]:gsub("start", "end"))

  -- The command's exit code is the wrapper's.
  local fail = shell.run({ prova.bin, "lock", "crunch", "--", "sh", "-c", "exit 7" },
    { cwd = pkg, merge_stderr = true })
  t:expect(fail.code, "exit codes forward"):equals(7)

  -- Grammar refusals: no token, no command, a package token with no package.
  t:expect(shell.run({ prova.bin, "lock" }, { cwd = pkg, merge_stderr = true }).code):equals(2)
  t:expect(shell.run({ prova.bin, "lock", "crunch" }, { cwd = pkg, merge_stderr = true }).code):equals(2)
  -- A NAMED directory: this one must have no package in it, and the scope's default directory is
  -- where `pkg` above wrote a prova.toml. The name says why it exists, and it is still reaped with
  -- the scope (agent-ergonomics.md#context-tempdir-not-idempotent).
  local homeless = shell.run({ prova.bin, "lock", "crunch", "--", "true" },
    { cwd = t:tempdir("homeless"), merge_stderr = true })
  t:expect(homeless.code, "a package lock needs a package"):equals(2)
  t:expect(homeless.stdout):contains("--machine")
end)

prova.test("xtask joins the same house rule, on the same file", {
  covers = "docs/design/architecture.md#locks-cross-instance",
  proves = "the cargo lock is a FILE, and every tool that agrees on the path joins the rule — xtask holds it by flocking the path directly rather than by calling prova, so nothing but agreement keeps them in the same queue. A drift here is silent: both tools keep working, they simply stop excluding each other, and the symptom lands in whichever conduct happens to be compiling when the other one builds",
}, function(t)
  local xtask = fs.read(prova.root .. "/xtask/src/main.rs")
  t:expect(xtask, "xtask flocks the package cargo lock"):contains(".prova/var/locks/cargo.lock")

  -- The path prova itself computes, asserted through the binary rather than by restating the
  -- convention here — a proof that hard-codes the same string twice proves only that it can copy.
  --
  -- Taken in a SCRATCH package, never in `prova.root`. The token is the real one, so the agreement
  -- being proven is the real one; but holding THIS repo's `cargo` lock made the proof deadlock by
  -- construction the moment it ran inside a conduct that already holds it — which is exactly what
  -- the coverage conduct does (`locks = { prova.writes("cargo") }`), so the black-box layer timed
  -- out here at 60s and no coverage could be measured at all. A proof of where a lock LIVES has no
  -- reason to queue for the one the suite is using.
  local probe = t:tempdir("lock-path-probe")
  fs.write(probe .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.mkdir(probe .. "/proofs")
  local from_prova = shell.run({ prova.bin, "lock", "cargo", "--", "true" },
    { cwd = probe, merge_stderr = true, timeout = "60s" })
  t:expect(from_prova.code, "`prova lock cargo` holds and releases: " .. from_prova.stdout):equals(0)
  -- …and it held the file at the path xtask hard-codes, resolved under whatever package it ran in.
  t:expect(probe .. "/.prova/var/locks/cargo.lock",
    "prova's computed path for the `cargo` token is the one xtask flocks"):exists()

  -- `xtask run` must NOT hold it: it delegates to prova, which asks for the same token, and a
  -- parent holding what its child needs is a deadlock rather than a slow build.
  t:expect(xtask, "the delegating command is exempt"):matches("Commands::Run%s*{%s*%.%.%s*}%s*=>%s*None")
end)

-- ── re-entrancy (docs/design/agent-ergonomics.md#a-lock-wrapper-can-wait-on-its-own-parent) ────
--
-- A flock excludes INDEPENDENT actors. A child inside `prova lock cargo -- …` is not one — it is
-- the work the parent took the lock to do — so queuing it behind its own parent is a deadlock
-- wearing mutual exclusion's clothes. Witnessed 2026-09-12: 22 minutes, killed by hand.

--- A package whose proof takes `writes(token)` and writes a marker, so a run that never acquires
--- is distinguishable from one that acquired instantly.
local function pkg_locking(t, name, token)
  local pkg = t:tempdir(name)
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/inner_test.lua", string.format([[
prova.test("the inner run takes the same token", { locks = { prova.writes(%q) } }, function(t)
  t:expect(1):equals(1)
end)
]], token))
  return pkg
end

prova.test("a child inherits its parent's hold instead of deadlocking behind it", {
  covers = "docs/design/agent-ergonomics.md#a-lock-wrapper-can-wait-on-its-own-parent",
  proves = "the composition the wrapper exists FOR — join the house rule around a command — and the most natural command to wrap is a prova run. Before this the inner run queued behind its own parent forever; the bound is the assertion, because a deadlock has no output to match on, only a clock that never stops",
}, function(t)
  local pkg = pkg_locking(t, "reentrant", "cargo")
  -- 60s is the SAFETY NET, not the expectation: this completes in under a second when the hold is
  -- inherited, and hangs forever when it is not. A generous bound keeps a slow CI runner honest
  -- while still turning the regression into a red rather than a wedged suite.
  local r = shell.run({ prova.bin, "lock", "cargo", "--", prova.bin },
    { cwd = pkg, merge_stderr = true, timeout = "60s" })

  t:expect(r.code, "the wrapped run completes:\n" .. r.stdout):equals(0)
  t:expect(r.stdout, "and says the hold was inherited rather than taken twice"):contains("inherited")
end)

prova.test("inheritance is matched on the resolved lock path, not the token name", {
  covers = "docs/design/agent-ergonomics.md#a-lock-wrapper-can-wait-on-its-own-parent",
  proves = "`cargo` at machine scope and `cargo` in a package are different contracts that share a name, so inheriting across them would hand out an exclusion nobody holds — the one way this change could INVENT a race instead of removing one. Substrate's wrapper was machine-scoped, which is precisely why the trap had never bitten before the day it did",
}, function(t)
  local pkg = pkg_locking(t, "scoped", "cargo")
  -- BOTH halves in one test, deliberately. Asserting only "no inheritance across scopes" is
  -- satisfied by an implementation that never inherits at all — it passed before a line of this
  -- existed. Pairing it with the same-scope case makes the absence mean something: the feature is
  -- demonstrably ON in the first run and demonstrably scoped in the second.
  local same = shell.run({ prova.bin, "lock", "cargo", "--", prova.bin },
    { cwd = pkg, merge_stderr = true, timeout = "60s" })
  t:expect(same.stdout, "same scope inherits:\n" .. same.stdout):contains("inherited")

  -- The wrapper holds the MACHINE `cargo`; the inner run declares the PACKAGE `cargo`. Different
  -- files, so the inner must take its own hold — free here, so this asserts inheritance did not
  -- apply rather than that it deadlocked.
  local cross = shell.run({ prova.bin, "lock", "cargo", "--machine", "--", prova.bin },
    { cwd = pkg, merge_stderr = true, timeout = "60s" })
  t:expect(cross.code, "the run still completes:\n" .. cross.stdout):equals(0)
  t:expect(cross.stdout, "but claims no inheritance across scopes"):never():contains("inherited")
end)

prova.test("a shared parent hold does not grant an exclusive child request", {
  covers = "docs/design/agent-ergonomics.md#a-lock-wrapper-can-wait-on-its-own-parent",
  proves = "the unsound upgrade. `--reads` is a CONCURRENT hold that several actors share, so treating it as covering a writer would let a child exclude nobody while believing it excludes everyone — a silent race manufactured by the very change meant to remove one",
}, function(t)
  local pkg = pkg_locking(t, "upgrade", "cargo")
  -- A second reader holds the same token for the duration, so the writer genuinely cannot proceed:
  -- if the upgrade were wrongly granted, the inner run would sail through and this goes red.
  local r = shell.run({
    "sh", "-c",
    '"$0" lock cargo --reads -- sh -c "sleep 6" & sleep 1; ' ..
    'PROVA_LOCK_WAIT_TIMEOUT=3s "$0" lock cargo --reads -- "$0"; echo "inner=$?"; wait',
    prova.bin,
  }, { cwd = pkg, merge_stderr = true, timeout = "90s" })

  -- The inner prova run wants writes("cargo") while two readers hold it: it must WAIT (and here,
  -- bounded, give up) rather than inherit the reader hold it was handed.
  t:expect(r.stdout, "the writer did not inherit a reader hold:\n" .. r.stdout)
    :never():contains("inner=0")
end)

prova.test("a queued leaf names who holds the lock, not 'another prova instance'", {
  covers = "docs/design/agent-ergonomics.md#a-lock-wrapper-can-wait-on-its-own-parent",
  proves = "22 minutes passed with the answer — your own parent — sitting in a record beside the lock file, because the queued line said the same sentence whatever the truth was. A message that cannot be wrong is a message that cannot help",
}, function(t)
  local pkg = pkg_locking(t, "named", "crunch")
  -- An outsider holds the token, so the inner run genuinely queues and must narrate a real holder.
  local r = shell.run({
    "sh", "-c",
    '"$0" lock crunch -- sh -c "sleep 5" & sleep 1; ' ..
    'PROVA_LOCK_WAIT_TIMEOUT=2s "$0" 2>&1 | head -40; wait',
    prova.bin,
  }, { cwd = pkg, merge_stderr = true, timeout = "90s" })

  t:expect(r.stdout, "the holder is named by pid:\n" .. r.stdout):matches("pid %d+")
  t:expect(r.stdout, "and the placeholder sentence is gone")
    :never():contains("another prova instance")
end)

-- ── the machine lock ADDRESS (docs/design/architecture.md#machine-lock-dir-follows-tmpdir) ──────

prova.test("a machine-wide hold is the same hold whatever $TMPDIR says", {
  covers = "docs/design/architecture.md#machine-lock-dir-follows-tmpdir",
  proves = "the contract's address cannot be derived from something its participants can disagree about. `temp_dir()` reads $TMPDIR, and anything that sets TMPDIR for its children — a cargo [env] table, a CI wrapper, a sandbox — forked 'one cargo at a time, machine-wide' into two rules that never saw each other. Silent by construction: both sides believe they hold it",
}, function(t)
  local a = t:tempdir("tmpdir-a")
  local b = t:tempdir("tmpdir-b")
  -- Two prova processes that disagree about TMPDIR as hard as possible, racing ONE machine token.
  -- Before the fix each took a hold in its own $TMPDIR/prova-locks and both sailed through.
  -- PROVA_SCRATCH_DIR isolates this from the developer's REAL machine lock directory. Without it
  -- the proof wrote `tmpsplit` into shared machine state and left it there — a test that pollutes
  -- the thing it is testing. It stays a valid discriminator: the two processes still disagree
  -- about TMPDIR as hard as possible, and under the old `temp_dir()` address they would still
  -- have split, because that code never consulted this variable.
  local sandbox = t:tempdir("machine-base")
  local r = shell.run({
    "sh", "-c",
    'TMPDIR="$1" "$0" lock tmpsplit --machine -- sh -c "sleep 4" & sleep 1; ' ..
    'TMPDIR="$2" PROVA_LOCK_WAIT_TIMEOUT=2s "$0" lock tmpsplit --machine -- true; ' ..
    'echo "second=$?"; wait',
    prova.bin, a, b,
  }, { env = { PROVA_SCRATCH_DIR = sandbox }, merge_stderr = true, timeout = "90s" })

  -- The second must NOT acquire: it is the same contract, so it queues and (bounded) gives up.
  t:expect(r.stdout, "the second process contends rather than taking a parallel hold:\n" .. r.stdout)
    :never():contains("second=0")
end)

prova.test("`prova locks` names the directory it consulted, even when empty", {
  covers = "docs/design/architecture.md#machine-lock-dir-follows-tmpdir",
  proves = "a split lock directory is invisible unless something prints the address. An empty scope is precisely when an operator is asking 'am I looking where the other process looked?', and the old output skipped that scope in silence",
}, function(t)
  local r = shell.run({ prova.bin, "locks", "--machine" }, { merge_stderr = true, timeout = "60s" })

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the machine scope names its directory:\n" .. r.stdout):contains("machine  (")
  -- And that directory is not the forkable one.
  t:expect(r.stdout, "…which is not $TMPDIR-derived"):never():contains("prova-locks")
end)
