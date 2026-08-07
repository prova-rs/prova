-- Spec: quality.gate — the golden-path composition. The posture selects the SURFACE: enforce authors
-- a proof (fatal), observe authors a reminder (surfaces, non-fatal until heeded). The check is a
-- fixed `limit=` or the committed baseline (measure.check). Black-box via prova.bin.

local function workspace(t, posture_line, baseline, gate_call)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n' .. (posture_line or ""))
  if baseline then
    fs.write(dir .. "/.prova/baselines/default.json", json.encode(baseline))
  end
  -- the gate (top-level), plus a noop proof so the suite is never empty
  fs.write(dir .. "/proofs/gate_test.lua",
    gate_call .. '\nprova.test("noop", function(t) t:expect(1):equals(1) end)\n')
  return dir
end

prova.test("enforce: a threshold violation fails the run (proof surface)", function(t)
  local dir = workspace(t, '[quality]\nposture = "enforce"\n', nil,
    'quality.gate{ name = "demo.size", value = 120, limit = 100 }')
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("over limit 100")
end)

prova.test("enforce: within the limit passes", function(t)
  local dir = workspace(t, '[quality]\nposture = "enforce"\n', nil,
    'quality.gate{ name = "demo.size", value = 80, limit = 100 }')
  t:expect(shell.run({ prova.bin }, { cwd = dir, merge_stderr = true }).code):equals(0)
end)

prova.test("observe: a violation surfaces (DUE) but does not fail the run until heeded", function(t)
  local dir = workspace(t, '[quality]\nposture = "observe"\n', nil,
    'quality.gate{ name = "demo.size", value = 120, limit = 100 }')
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0) -- observe surface = reminder = non-fatal
  t:expect(r.stdout):contains("over limit 100") -- but the reason is surfaced
  local heeded = shell.run({ prova.bin, "--heed" }, { cwd = dir, merge_stderr = true })
  t:expect(heeded.code):never():equals(0) -- heed promotes the DUE reminder to a failure
end)

prova.test("explicit enforce= overrides an observe posture", function(t)
  local dir = workspace(t, '[quality]\nposture = "observe"\n', nil,
    'quality.gate{ name = "demo.size", value = 120, limit = 100, enforce = true }')
  t:expect(shell.run({ prova.bin }, { cwd = dir, merge_stderr = true }).code):never():equals(0)
end)

prova.test("baseline mode: a regression past the committed baseline fails under enforce", function(t)
  local dir = workspace(t, '[quality]\nposture = "enforce"\n',
    { schema = 1, metrics = { ["demo.size"] = { value = 100, direction = "lower_is_better" } } },
    'quality.gate{ name = "demo.size", value = 120 }')
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("regressed to 120")
end)
