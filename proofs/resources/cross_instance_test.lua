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
