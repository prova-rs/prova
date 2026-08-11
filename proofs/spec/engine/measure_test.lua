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

-- A workspace whose one proof records `demo.size`; helpers to bank and read the floor back.
local function banking_workspace(t)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", MANIFEST)
  local baseline_path = dir .. "/.prova/baselines/default.json"
  local w = { dir = dir }
  function w.record(v)
    fs.write(dir .. "/proofs/gate_test.lua",
      'prova.test("record", function(t) measure.record("demo.size", ' .. v .. ', { direction = "lower_is_better" }) end)\n')
  end
  function w.bank(flag)
    return shell.run({ prova.bin, flag or "--update-baseline" }, { cwd = dir, merge_stderr = true })
  end
  function w.metric()
    return json.decode(fs.read(baseline_path)).metrics["demo.size"]
  end
  function w.edit(f)
    local base = json.decode(fs.read(baseline_path))
    f(base.metrics["demo.size"])
    fs.write(baseline_path, json.encode(base))
  end
  return w
end

prova.test("bare --update-baseline establishes first sight, then HOLDS a goal-less metric",
  { covers = "docs/design/verifiers.md#baseline-bank-policy" }, function(t)
  local w = banking_workspace(t)

  -- Establish: a metric with no floor gates nothing, so first sight always writes.
  w.record(100)
  local r = w.bank()
  t:expect(r.code, r.stdout):equals(0)
  t:expect(w.metric().value):equals(100)

  -- Steady state: an improvement on a goal-less metric stays green and UNBANKED — a lucky run
  -- never mints a floor nobody chose — and the report names the deliberate-banking spelling.
  w.record(80)
  local r2 = w.bank()
  t:expect(r2.stdout):contains("held")
  t:expect(r2.stdout):contains("--update-baseline=demo.size")
  t:expect(w.metric().value):equals(100)
end)

prova.test("a goal-carrying metric tightens on the bare flag — active debt banks, and the goal survives",
  { covers = "docs/design/verifiers.md#baseline-bank-policy" }, function(t)
  local w = banking_workspace(t)
  w.record(100)
  w.bank()
  w.edit(function(m) m.goal = 50 end)

  w.record(80)
  local r = w.bank()
  t:expect(r.stdout):contains("tightened")
  t:expect(w.metric().value):equals(80)
  t:expect(w.metric().goal, "banking a gain never retires the goal that demanded it"):equals(50)

  -- The loosen guard is absolute on every flag path.
  w.record(150)
  local r2 = w.bank()
  t:expect(r2.stdout):contains("REFUSED")
  t:expect(r2.stdout):contains("hand-edit")
  t:expect(w.metric().value):equals(80)
end)

prova.test("named banking moves exactly the named metric, and a typo is loud, never a silent no-op",
  { covers = "docs/design/verifiers.md#baseline-bank-policy" }, function(t)
  local w = banking_workspace(t)
  w.record(100)
  w.bank()

  -- Goal-less, but NAMED: the human asked, so it moves.
  w.record(80)
  local r = w.bank("--update-baseline=demo.size")
  t:expect(r.stdout):contains("tightened")
  t:expect(w.metric().value):equals(80)

  -- A selector matching nothing recorded this run is a refusal with the selector named.
  w.record(70)
  local r2 = w.bank("--update-baseline=demo.syze")
  t:expect(r2.stdout):contains("REFUSED")
  t:expect(r2.stdout):contains("demo.syze")
  t:expect(w.metric().value, "nothing moved on the typo"):equals(80)
end)

-- The tolerance contract, authored ahead of implementation (spec-first). Found live: the layered
-- coverage conduct banks each layer at its best-ever, but the BLACK-BOX layer wobbles ~0.3% per
-- run (timing-dependent paths, retry arms taken or not), so a peak-banked hard floor flakes on
-- honest runs. A noisy metric needs to DECLARE its noise: `tolerance` in the committed baseline,
-- with the gate holding `floor - tolerance` while banking still records the best-seen value.
prova.test("a baseline `tolerance` absorbs declared noise without loosening the banked floor", {
  proves = "north-star arc: the blackbox coverage ratchet flaked on run-to-run noise, and re-flaked after every bank re-peaked the floor (2026-08-10) — a noisy metric declares its band instead",
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
