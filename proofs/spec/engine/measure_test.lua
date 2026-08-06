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
