--- The first-byte bound (docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently):
--- the third clock, answering a question neither of the others can.
---
--- `idle_timeout` asks "is it still alive?" — and a wedged builder burning no CPU while a healthy
--- one goes quiet mid-step makes that answer ambiguous. A wall `timeout` must be sized for the
--- slowest honest build, so it answers hours late. Time-to-FIRST-byte is the interval a caller can
--- bound tightly WITHOUT knowing the work: a tool that has said nothing has not started.
---
--- Every conduct-runtime case drives the SUBJECT through `prova.bin eval` — called directly in this
--- file, `shell.run` would exercise whichever prova is CONDUCTING the suite.

local function subject_eval(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "30s" })
end

prova.test("a command that never speaks dies at the first-byte bound, named as never-started", {
  covers = "docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently",
  proves = "the wedge's signature is total silence, and the operator's next move depends on hearing it as such: 'timed out' sends them to raise the budget (which is how one suite spent 2h per invocation), 'never answered' sends them to restart the builder",
}, function(t)
  local r = subject_eval([[
local ok, err = pcall(function()
  return shell.run("sleep 20; echo too-late", { first_byte = "400ms" })
end)
print("killed=" .. tostring(not ok))
print(tostring(err))
]])
  t:expect(r.stdout, "the mute conduct is killed"):contains("killed=true")
  t:expect(r.stdout, "the finding is the silence itself"):contains("no output at all")
  t:expect(r.stdout, "…the bound that fired"):contains("first_byte")
  t:expect(r.stdout, "…and the diagnosis, not merely the symptom"):contains("never started")
end)

prova.test("the first byte disarms the bound for good — a slow talker is not a wedge", {
  covers = "docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently",
  proves = "the bound would be useless if it were a rate: a build that prints its plan in one second and then compiles silently for an hour is the NORMAL case, so the question 'did it ever start' must be answered once and never re-asked",
}, function(t)
  local r = subject_eval([[
local r = shell.run("echo started; sleep 2; echo done", { first_byte = "400ms" })
print("code=" .. r.code)
print(r.stdout)
]])
  t:expect(r.stdout, "a conduct that spoke once survives silence 5× the bound"):contains("code=0")
  t:expect(r.stdout, "…to completion"):contains("done")
end)

prova.test("the three clocks compose, and the earliest true answer wins", {
  covers = "docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently",
  proves = "a wedge under a realistic wall budget is exactly the shipped failure — the outer bound was correct and useless because it was sized for the honest worst case; first_byte must fire first, and must be distinguishable in the report from the timeout that would have followed",
}, function(t)
  local r = subject_eval([[
local start = os.time()
local ok, err = pcall(function()
  return shell.run("sleep 25", { first_byte = "400ms", idle_timeout = "20s", timeout = "20s" })
end)
print("killed=" .. tostring(not ok))
print("elapsed=" .. tostring(os.time() - start))
print(tostring(err))
]])
  t:expect(r.stdout, "killed=true"):contains("killed=true")
  t:expect(r.stdout, "the first-byte clock answered, not the wall clock"):contains("first_byte")
  t:expect(r.stdout, "…and not as a timeout"):never():contains("timed out after")
  -- Seconds, not the 20s the wall bound would have taken: the whole point is answering early.
  local elapsed = tonumber(r.stdout:match("elapsed=(%d+)"))
  t:expect(elapsed, "it answered in seconds, far inside the wall budget"):lt(10)
end)

prova.test("a first-byte kill reaps the conduct's whole tree, like every other bound", {
  covers = "docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently",
  proves = "a new bound is a new way to leak: `docker build` spawns helpers, and a bound that only abandons the wait would leave the very builder processes the report just called dead (docs/design/verifiers.md#timeout-reaps-the-conduct is the standing rule, and it holds per-clock, not per-module)",
}, function(t)
  local token = "77.221"
  -- `77.221` probes as `77[.]221`: on Linux pgrep -f matches the wrapping shell's own argv, so a
  -- literal pattern finds the probe itself (the release gate caught this once already).
  local pattern = (token:gsub("%.", "[.]"))
  subject_eval('pcall(function() return shell.run("(sleep ' .. token
    .. ' &); sleep 20", { first_byte = "400ms" }) end); return "done"')
  shell.run("sleep 0.5") -- let the group kill settle past reaping races
  local alive = shell.run("pgrep -f 'sleep " .. pattern .. "'").code == 0
  shell.run("pkill -f 'sleep " .. pattern .. "' 2>/dev/null; true")
  t:expect(alive, "the backgrounded grandchild died with the muted conduct"):is_false()
end)

prova.test("docker.build carries the bound by default, and the error names the builder's cure", {
  covers = "docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently",
  requires = { "docker" },
  proves = "the bound has to live where the wedge was met, and it has to teach the fix: a green `docker info` (the capability probe) does NOT clear a wedged buildkitd, so an operator reading only 'no output' would conclude the daemon is fine and the build is slow — which is what happened",
}, function(t)
  -- Through the SUBJECT: `docker.build` called in this body would exercise whichever prova is
  -- conducting the suite, and a conductor without the bound silently ignores the option — which is
  -- exactly how this proof first passed while proving nothing.
  local r = subject_eval([[
local dir = fs.tempdir()
fs.write(dir .. "/Dockerfile", "FROM alpine:3.20\nRUN true\n")
-- No healthy builder can answer in 1ms, so this induces the wedge's own verdict on a working
-- daemon — the only honest way to reach the docker.build path black-box.
local ok, err = pcall(function()
  return docker.build{ context = dir, first_byte = "1ms" }
end)
print("built=" .. tostring(ok))
print(tostring(err))
]])
  t:expect(r.stdout, "an unanswering builder fails the build"):contains("built=false")
  t:expect(r.stdout, "the silence is the finding"):contains("no output at all")
  t:expect(r.stdout, "…named as a wedge, not a slow build"):contains("wedged builder")
  t:expect(r.stdout, "…with the cure"):contains("Restart")
  t:expect(r.stdout, "…and the reason a green capability probe does not clear it"):contains("docker pull")
end)

prova.test("the default bound does not touch a real build — the negative control", {
  covers = "docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently",
  requires = { "docker" },
  proves = "a start-up bound applied one clock too eagerly turns every image-building suite red on a cold daemon; the value only holds if an ordinary build under the DEFAULT is untouched",
}, function(t)
  local r = subject_eval([[
local dir = fs.tempdir()
fs.write(dir .. "/Dockerfile", "FROM alpine:3.20\nRUN echo built\n")
print("ref=" .. tostring(docker.build{ context = dir }))
]])
  t:expect(r.stdout, "the build completed under the default first-byte bound"):contains("ref=prova-build-")
end)
