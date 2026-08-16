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
  local from_prova = shell.run({ prova.bin, "eval",
    'local d = fs.tempdir(); local r = shell.run({ prova.bin, "lock", "cargo", "--", "true" }, ' ..
    '{ cwd = prova.root }); return tostring(r.code)' },
    { merge_stderr = true, timeout = "60s" })
  t:expect(from_prova.code, "`prova lock cargo` holds and releases: " .. from_prova.stdout):equals(0)

  -- `xtask run` must NOT hold it: it delegates to prova, which asks for the same token, and a
  -- parent holding what its child needs is a deadlock rather than a slow build.
  t:expect(xtask, "the delegating command is exempt"):matches("Commands::Run%s*{%s*%.%.%s*}%s*=>%s*None")
end)
