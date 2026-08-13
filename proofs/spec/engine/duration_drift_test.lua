--- Duration drift as attention (docs/design/reminders.md#duration-drift-is-attention): the
--- timing capability is condition-surface data — `account.duration_ms`, the auto-recorded
--- `run.duration_ms` measurement, and `account.baselines` — and a drift policy composed over it
--- is passive until a context heeds it. Slowness is attention; only `--heed` makes it death.

local scaffold = require("scaffold")

local function package(t, reminder)
  return scaffold.package(t, { proofs = { ["paced_test.lua"] = [[
prova.test("does a little work", function(t)
  shell.run("sleep 0.05")
  t:expect(true):is_true()
end)
]] .. reminder } })
end

prova.test("a slow run trips the drift watcher — and stays green until heeded", {
  covers = "docs/design/reminders.md#duration-drift-is-attention",
  proves = "the whole posture: 'now taking too long' reports as DUE attention while the run stays green (slowness is not a defect in the change under test), and the SAME declaration becomes a gate only when a context says --heed — no constant in the engine, no forced failure",
}, function(t)
  local proj = package(t, [[
prova.remind("too-slow", { tags = { "timings" }, when = function(a)
  return a.duration_ms > 10 and ("took " .. math.floor(a.duration_ms) .. "ms")
end }, "the run outgrew the believable window")
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, "DUE is attention, not death"):equals(0)
  t:expect(r.stdout, "…but it is LOUD attention"):contains("too-slow")
  t:expect(r.stdout, "the condition's why names the observation"):contains("took ")

  local heeded = shell.run(prova.bin .. " --heed=timings", { cwd = proj, merge_stderr = true })
  t:expect(heeded.code, "a context that heeds turns the same watcher into a gate"):never():equals(0)
end)

prova.test("the run's duration is a recorded measurement, and banked baselines reach conditions", {
  covers = "docs/design/reminders.md#duration-drift-is-attention",
  proves = "no hard-coded limitation anywhere in the chain: the run records run.duration_ms like any metric, banking it is the deliberate act, and the drift policy compares the two — unbanked, the watcher holds quietly instead of guessing a constant",
}, function(t)
  local proj = package(t, [[
prova.remind("duration-drift", { when = function(a)
  local seen = a.measurements["run.duration_ms"]
  if not seen then return "run.duration_ms is not in the account" end
  local banked = a.baselines["run.duration_ms"]
  if not banked then return false end
  return a.duration_ms > banked * 1.5
    and ("ran " .. math.floor(a.duration_ms) .. "ms against banked " .. math.floor(banked) .. "ms")
end }, "the run outgrew its banked duration")
]])
  -- Unbanked: the measurement is present (the condition would go DUE if it were missing), the
  -- baseline is absent, and the watcher holds.
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "present measurement + absent baseline = quiet watch"):never():contains("duration-drift")

  -- Bank a tiny duration by hand (the deliberate act, made explicit), and the drift trips.
  fs.mkdir(proj .. "/.prova/baselines")
  fs.write(proj .. "/.prova/baselines/timings.json",
    '{"schema":1,"metrics":{"run.duration_ms":{"value":1,"direction":"lower_is_better"}}}')
  local drifted = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(drifted.code, "still green — passive"):equals(0)
  t:expect(drifted.stdout, "…and DUE, naming both numbers"):contains("against banked 1ms")
end)
