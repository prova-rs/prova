--- Liveness supervision for conducts (docs/design/verifiers.md#conduct-heartbeat-not-deadline):
--- `idle_timeout` bounds SILENCE, never work. A slow-but-alive command outlives any point where
--- a same-sized wall clock would have killed it; a silent hang dies at the idle bound, faster
--- than any honest deadline; and the wall-clock `timeout` still composes as the outer bound.
---
--- Every scenario drives the SUBJECT's shell.run via `prova.bin eval` — the feature under test
--- lives in the runtime, and the conductor executing this file may be any prova. Subjects are
--- `sh` loops: externally-sized work with a heartbeat, the conduct shape itself.

--- Run a snippet in the subject's full environment; returns the outer result. The outer wall
--- bound keeps every failure mode finite — a broken idle kill turns into a bounded, named red.
local function subject_eval(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "20s" })
end

prova.test("a slow-but-alive command outlives an idle bound smaller than its total runtime", {
  covers = "docs/design/verifiers.md#conduct-heartbeat-not-deadline",
  proves = "the field report's false failure: a steadily-progressing nextest conduct was killed at its wall budget. Liveness must measure silence between bytes, not total duration — a command alive for 3× its idle bound proves the clock resets on output",
}, function(t)
  -- ~1.2s total, one heartbeat every ~0.15s: total >> idle_timeout, every gap << idle_timeout.
  local r = subject_eval([[
local r = shell.run("for i in 1 2 3 4 5 6 7 8; do echo beat $i; sleep 0.15; done",
  { idle_timeout = "500ms" })
print("code=" .. r.code)
print(r.stdout)
]])
  t:expect(r.stdout, "steady progress is never killed"):contains("code=0")
  t:expect(r.stdout):contains("beat 8")
end)

prova.test("a silent-but-busy command survives: CPU is the heartbeat when the pipes are quiet", {
  covers = "docs/design/verifiers.md#conduct-heartbeat-not-deadline",
  proves = "the false kill that shipped and was caught same-day: a big crate's codegen says nothing for minutes while working flat-out, so silence on the pipes is only half the evidence — and the reader is native (procfs/libproc/GetProcessTimes), never a ps dialect guess, or portability would re-import the same disease one tool over",
}, function(t)
  -- ~1.5s of pipe silence, CPU accruing the whole time (the $(date) forks bill the shell),
  -- against a 500ms idle bound: three windows expire with no bytes and the child must live.
  local r = subject_eval([[
local r = shell.run("deadline=$(( $(date +%s) + 2 )); while [ $(date +%s) -lt $deadline ]; do :; done; echo finished",
  { idle_timeout = "500ms" })
print("code=" .. r.code)
print(r.stdout)
]])
  t:expect(r.stdout, "busy silence is life"):contains("code=0")
  t:expect(r.stdout):contains("finished")
end)

prova.test("a silent hang dies at the idle bound, and the error names the silence", {
  covers = "docs/design/verifiers.md#conduct-heartbeat-not-deadline",
  proves = "a genuine hang is caught FASTER than any honest deadline: the author no longer prices the whole build, only how long silence is believable — and the failure must read as 'went quiet', with the tail, never as a generic timeout",
}, function(t)
  -- Prints once, then hangs silently far past the idle bound. The inner command would run 30s;
  -- the 20s outer wall bound means a broken kill fails THIS proof loudly, never hangs the suite.
  local r = subject_eval([[
local ok, err = pcall(function()
  return shell.run("echo started; sleep 30", { idle_timeout = "400ms" })
end)
print("killed=" .. tostring(not ok))
print(tostring(err))
]])
  t:expect(r.stdout, "the conduct is killed"):contains("killed=true")
  t:expect(r.stdout, "the error names the silence, not a budget"):contains("no output")
  t:expect(r.stdout, "…and the absent CPU evidence — dead means dead, not busy"):contains("no CPU progress")
  t:expect(r.stdout, "…and the bound that fired"):contains("idle_timeout")
  t:expect(r.stdout, "…and carries the tail, so the stall point is in the report"):contains("started")
end)

--- Run a bounded sleep in the SUBJECT, wait out the bound, and answer whether the child
--- outlived the report. The token makes the process findable; strays are reaped either way,
--- so a red here never litters the host.
local function leaks_after(bounds, token)
  local snippet = 'pcall(function() return shell.run("sleep ' .. token .. '", ' .. bounds
    .. ') end); return "done"'
  shell.run({ prova.bin, "eval", snippet }, { merge_stderr = true, timeout = "20s" })
  shell.run("sleep 0.5") -- let the kill (or the leak) settle past reaping races
  -- `38.111` probes as `38[.]111`: Linux pgrep -f sees the wrapping shell's own argv, so a
  -- literal pattern matches the probe itself (caught live by the release gate).
  local esc = token:gsub("%.", "[.]")
  local alive = shell.run("pgrep -f 'sleep " .. esc .. "'").code == 0
  shell.run("pkill -f 'sleep " .. esc .. "' 2>/dev/null; true")
  return alive
end

prova.test("a bound that fires kills the conduct — wall, idle, and composed alike", {
  covers = "docs/design/verifiers.md#timeout-reaps-the-conduct",
  proves = "a bound that only abandons the wait reports red while the child keeps holding the locks the report just implied were free — the observed shape was an orphaned nextest wedging the next invocation's cargo. Dead means dead on every bound, not just the new ones",
}, function(t)
  t:expect(leaks_after('{ timeout = "300ms" }', "38.111"), "the wall clock reaps"):is_false()
  t:expect(leaks_after('{ idle_timeout = "300ms" }', "38.222"), "the idle clock reaps"):is_false()
  t:expect(leaks_after('{ idle_timeout = "10s", timeout = "300ms" }', "38.333"),
    "the composed outer bound reaps"):is_false()
end)

prova.test("the wall clock composes as the outer bound over a live-but-endless conduct", {
  covers = "docs/design/verifiers.md#conduct-heartbeat-not-deadline",
  proves = "liveness alone would let a chatty-forever command run unbounded — the two bounds answer different questions (is it dead? / may it keep going?) and must compose, the wall clock firing even while the heartbeat is healthy",
}, function(t)
  local r = subject_eval([[
local ok, err = pcall(function()
  return shell.run("while true; do echo alive; sleep 0.1; done",
    { idle_timeout = "1s", timeout = "700ms" })
end)
print("killed=" .. tostring(not ok))
print(tostring(err))
]])
  t:expect(r.stdout, "the outer bound still kills"):contains("killed=true")
  t:expect(r.stdout, "…as the wall clock, not the heartbeat"):contains("timed out")
end)
