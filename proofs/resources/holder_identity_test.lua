-- A hold is nameable (docs/design/architecture.md#a-hold-names-its-holder). The flock is
-- anonymous by construction — the kernel knows the holder, nothing readable does — so a waiter
-- and an operator both had exactly one diagnostic available: `ps`. These prove the surfaces that
-- replaced it, against REAL contending processes, because none of this can be simulated
-- in-process: a flock excludes descriptions, so one process holding its own lock proves nothing.
--
-- Deliberately NOT proved here: that a hung HOLDER is bounded. It is not — the hold-side
-- supervision is docs/design/agent-ergonomics.md#a-hung-holder-never-releases, still open — and a
-- proof implying otherwise would be worse than no proof.

--- A machine-wide token unique to this test, so a concurrent agent's run can never contend with
--- ours (machine locks live in a box-wide directory — a fixed name would be a shared secret).
local function token(t, suffix)
  return "provahold-" .. t:tempdir():match("([^/\\]+)$") .. "-" .. suffix
end

--- The token, safe to embed in a Lua pattern. Tokens here are full of `-`, which is a QUANTIFIER
--- in Lua patterns, not a literal — so an unescaped token matches nothing and every assertion
--- fails for a reason that has nothing to do with locks.
local function pat(s)
  return (s:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0"))
end

--- What `prova locks --machine` says right now.
local function survey()
  return shell.run({ prova.bin, "locks", "--machine" }, { merge_stderr = true, timeout = "30s" }).stdout
end

--- Hold `tok` for `seconds` in a detached prova, returning once it REALLY holds. Polling the
--- survey rather than sleeping is the difference between a slow start costing time and costing a
--- false failure — and it is the same observe-until-true rule the suite imposes elsewhere.
local function holder(tok, seconds)
  local proc = shell.spawn({ prova.bin, "lock", tok, "--machine", "--", "sleep", tostring(seconds) })
  prova.retry(function()
    return survey():match("HELD%s+" .. pat(tok)) ~= nil
  end, { timeout = "20s", every = "100ms", message = "the holder never took " .. tok })
  return proc
end

prova.test("`prova locks` names the pid and command behind a held token", {
  covers = "docs/design/architecture.md#a-hold-names-its-holder",
  proves = "the whole incident: a `cargo` token held 1 d 22 h by a hung conduct, and every waiter behind it could say only that it was waiting. A flock records no holder, so 'what is my build queued behind?' had no answer that did not start with `ps`",
}, function(t)
  local tok = token(t, "named")
  local hold = holder(tok, 4)

  local out = survey()
  t:expect(out, "the token reads held"):matches("HELD%s+" .. pat(tok))
  t:expect(out, "…by a writer with a pid"):matches("writer pid %d+")
  t:expect(out, "…naming what it is doing:\n" .. out):contains("lock " .. tok)
  t:expect(out, "…and where the file that IS the contract lives"):contains("prova-locks")

  hold:wait()
  -- Release takes the record with it. A record outliving its flock would accuse a pid that
  -- excludes nobody on every later read, which is worse than saying nothing at all.
  local after = survey()
  t:expect(after, "the released token reads free"):matches("free%s+" .. pat(tok))
  t:expect(after:match("pid %d+ — [^\n]*" .. pat(tok)) == nil, "…and names nobody:\n" .. after):is_true()
end)

prova.test("a queued wait names the holder, and keeps saying so while it waits", {
  covers = "docs/design/architecture.md#a-hold-names-its-holder",
  proves = "a 14 h wait printed one line — 'waiting for lock (held elsewhere)' — and then nothing, because the blocking flock owned the thread that would have spoken. Silence for the duration is what turned a diagnosable queue into a mystery: the run banks lock_wait_ms only if it FINISHES",
}, function(t)
  local tok = token(t, "narrated")
  local hold = holder(tok, 4)

  -- A narration cadence the proof can outlast. That the DEFAULT is 60s is a unit-test assertion;
  -- what needs proving black-box is that the repeat happens at all, which at 60s would mean a
  -- minute-long proof for one line.
  local r = shell.run({ prova.bin, "lock", tok, "--machine", "--", "true" },
    { merge_stderr = true, timeout = "60s", env = { PROVA_LOCK_NARRATE_EVERY = "1s" } })
  hold:wait()

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the first line names the holder, not just the queue:\n" .. r.stdout)
    :matches("waiting for lock[^\n]*pid %d+")
  t:expect(r.stdout, "…and what it is queued behind"):contains("lock " .. tok)
  local repeats = select(2, r.stdout:gsub("still waiting", ""))
  t:expect(repeats, "a long wait re-says what it waits on:\n" .. r.stdout):gte(1)
end)

prova.test("a bounded wait gives up naming the holder, the elapsed time, and the way out", {
  covers = "docs/design/architecture.md#a-hold-names-its-holder",
  proves = "in CI the wait ends when the RUNNER's timeout fires, which kills the process with no line naming the token, the holder, or the elapsed time. A bound prova owns can fail with all three — and must name the escape, because nothing can release another process's flock",
}, function(t)
  local tok = token(t, "bounded")
  local hold = holder(tok, 5)

  local r = shell.run({ prova.bin, "lock", tok, "--machine", "--wait-timeout", "1s", "--", "true" },
    { merge_stderr = true, timeout = "60s" })
  t:expect(r.code, "giving up is a failure, not a silent skip:\n" .. r.stdout):equals(2)
  t:expect(r.stdout, "it says it gave up, with how long it waited"):matches("gave up after [%d%.]+s")
  t:expect(r.stdout, "…names the holder"):matches("pid %d+")
  t:expect(r.stdout, "…and names the dial, since the holder is not ours to reap")
    :contains("PROVA_LOCK_WAIT_TIMEOUT")

  -- Abandoning must not leak the hold: the helper thread still blocking on the flock wins it the
  -- moment the incumbent leaves, and has to let go rather than become a holder nobody awaits.
  hold:wait()
  local freed = shell.run({ prova.bin, "lock", tok, "--machine", "--wait-timeout", "10s", "--", "true" },
    { merge_stderr = true, timeout = "60s" })
  t:expect(freed.code, "the abandoned wait leaked a hold:\n" .. freed.stdout):equals(0)
end)

prova.test("a holder with no record reads as held, never as free", {
  covers = "docs/design/architecture.md#a-hold-names-its-holder",
  proves = "the lock file is a PUBLIC convention — xtask, a Makefile, flock(1) — so a holder that never heard of the record format is normal, not broken. Inferring 'free' from 'no record' would make the most common external holder invisible: the fail-open direction, and the exact defect the run-state record just paid down",
}, function(t)
  local tok = token(t, "foreign")
  local hold = holder(tok, 5)

  -- The lock directory is printed by the survey precisely so it can be addressed; deleting the
  -- records beside a still-held flock reproduces an outside holder exactly — the flock is held,
  -- and nothing on disk claims it.
  local dir = survey():match("machine%s+%((.-)%)")
  t:expect(dir, "the survey prints the directory that IS the contract"):is_truthy()
  fs.remove_all(dir .. "/" .. tok .. ".holders")

  local out = survey()
  t:expect(out, "the kernel is authority: still held"):matches("HELD%s+" .. pat(tok))
  t:expect(out, "…and the record's absence is stated, not inferred away"):contains("unregistered holder")

  -- A waiter behind an anonymous holder still gets an honest line rather than a bare "waiting".
  local r = shell.run({ prova.bin, "lock", tok, "--machine", "--wait-timeout", "1s", "--", "true" },
    { merge_stderr = true, timeout = "60s" })
  t:expect(r.stdout, "the wait says what it does know:\n" .. r.stdout):contains("unregistered holder")
  hold:wait()
end)
