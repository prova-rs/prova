-- Spec for the measurements / ratchet core (docs/design/verifiers.md). Black-box: each case builds a
-- throwaway workspace with its own committed baseline and drives it through prova.bin, so prova.root
-- inside is the tempdir and the ratchet reads that workspace's .prova/baselines/. This proves the
-- gate's teeth (a regression is red, a missing baseline is red) and the guard (--update-baseline
-- establishes and tightens, but refuses to loosen).

local MANIFEST = '[run]\nproofs = ["proofs"]\n'

-- A workspace with an optional baseline for the `default` set and one proof body. Returns its dir.
local function workspace(t, baseline, body)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", MANIFEST)
  if baseline then
    fs.write(dir .. "/.prova/baselines/default.json", json.encode(baseline))
  end
  fs.write(dir .. "/proofs/gate_test.lua", body)
  return dir
end

local CEILING_100 = { schema = 1, metrics = { ["demo.size"] = { value = 100, direction = "lower_is_better" } } }

prova.test("ratchet passes at or under the baseline ceiling (lower is better)", function(t)
  local dir = workspace(t, CEILING_100, [[
    prova.test("at ceiling", function(t) measure.ratchet(t, "demo.size", 100) end)
    prova.test("improved",   function(t) measure.ratchet(t, "demo.size", 80) end)
  ]])
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
end)

prova.test("ratchet fails when the value regresses past the ceiling", function(t)
  local dir = workspace(t, CEILING_100, [[
    prova.test("regressed", function(t) measure.ratchet(t, "demo.size", 150) end)
  ]])
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("regressed to 150")
end)

prova.test("ratchet refuses to pass with no committed baseline", function(t)
  local dir = workspace(t, nil, [[
    prova.test("no floor", function(t) measure.ratchet(t, "demo.size", 50) end)
  ]])
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("no baseline")
end)

prova.test("higher-is-better ratchet fails below the floor, passes at or above it", function(t)
  local base = { schema = 1, metrics = { ["demo.cov"] = { value = 90, direction = "higher_is_better" } } }
  local below = workspace(t, base, [[
    prova.test("dropped", function(t) measure.ratchet(t, "demo.cov", 85, { direction = "higher_is_better" }) end)
  ]])
  t:expect(shell.run({ prova.bin }, { cwd = below, merge_stderr = true }).code):never():equals(0)
  local held = workspace(t, base, [[
    prova.test("held", function(t) measure.ratchet(t, "demo.cov", 95, { direction = "higher_is_better" }) end)
  ]])
  t:expect(shell.run({ prova.bin }, { cwd = held, merge_stderr = true }).code):equals(0)
end)

prova.test("--update-baseline establishes, tightens, and refuses to loosen", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", MANIFEST)
  local baseline_path = dir .. "/.prova/baselines/default.json"
  local function record(v)
    fs.write(dir .. "/proofs/gate_test.lua",
      'prova.test("record", function(t) measure.record("demo.size", ' .. v .. ', { direction = "lower_is_better" }) end)\n')
  end
  local function value()
    return json.decode(fs.read(baseline_path)).metrics["demo.size"].value
  end

  -- Establish: no baseline yet, so the first update writes the observed value.
  record(100)
  local r1 = shell.run({ prova.bin, "--update-baseline" }, { cwd = dir, merge_stderr = true })
  t:expect(r1.code, r1.stdout):equals(0)
  t:expect(baseline_path):exists()
  t:expect(value()):equals(100)

  -- Tighten: an improvement moves the floor down freely.
  record(80)
  shell.run({ prova.bin, "--update-baseline" }, { cwd = dir, merge_stderr = true })
  t:expect(value()):equals(80)

  -- Refuse: a regression is NOT written — the committed floor holds, and the guard says so.
  record(150)
  local r3 = shell.run({ prova.bin, "--update-baseline" }, { cwd = dir, merge_stderr = true })
  t:expect(r3.stdout):contains("REFUSED")
  t:expect(value()):equals(80)
end)

-- The tolerance contract, authored ahead of implementation (spec-first). Found live: the layered
-- coverage conduct banks each layer at its best-ever, but the BLACK-BOX layer wobbles ~0.3% per
-- run (timing-dependent paths, retry arms taken or not), so a peak-banked hard floor flakes on
-- honest runs. A noisy metric needs to DECLARE its noise: `tolerance` in the committed baseline,
-- with the gate holding `floor - tolerance` while banking still records the best-seen value.
prova.test("a baseline `tolerance` absorbs declared noise without loosening the banked floor", {
  promises = "north-star arc: the blackbox coverage ratchet flaked on run-to-run noise (2026-08-10)",
}, function(t)
  local dir = t:tempdir()
  fs.mkdir(dir .. "/proofs")
  fs.write(dir .. "/prova.toml", "[run]\nproofs = [\"proofs\"]\n")
  local baseline_dir = dir .. "/.prova/baselines"
  fs.mkdir(baseline_dir)
  -- A committed floor of 80 with a declared noise band of 2.
  fs.write(baseline_dir .. "/default.json", json.encode({
    schema = 1,
    metrics = { ["demo.coverage"] = { value = 80, direction = "higher_is_better", tolerance = 2 } },
  }))
  local function gate(v)
    fs.write(dir .. "/proofs/gate_test.lua",
      'prova.test("gate", function(t) measure.ratchet(t, "demo.coverage", ' .. v ..
      ', { direction = "higher_is_better" }) end)\n')
    return shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  end
  -- Within the declared band: green, and the floor is NOT rewritten.
  t:expect(gate(78.5).code, "78.5 is inside floor-2"):equals(0)
  -- Past the band: the ratchet is as hard as ever.
  t:expect(gate(77.5).code, "77.5 is a real regression"):never():equals(0)
end)
