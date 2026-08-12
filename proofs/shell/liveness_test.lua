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
  t:expect(r.stdout, "…and the bound that fired"):contains("idle_timeout")
  t:expect(r.stdout, "…and carries the tail, so the stall point is in the report"):contains("started")
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
