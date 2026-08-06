-- Spec for paydown mode (docs/design/verifiers.md): a baseline metric with a `goal` drives the value
-- down proactively, not just holding a ceiling. The ceiling still applies; on top of it a met goal
-- graduates (lock in + retire it, prova's idiom) and a past `deadline` turns the standing debt red.
-- Black-box via prova.bin so a failing assertion shows as a non-zero exit.

local MANIFEST = '[run]\nproofs = ["proofs"]\n'

-- A workspace whose baseline sets demo.size ceiling 100 plus a paydown goal (and optional deadline),
-- and whose proof ratchets a given value.
local function workspace(t, goal, deadline, value)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", MANIFEST)
  local metric = { value = 100, direction = "lower_is_better", goal = goal }
  if deadline then metric.deadline = deadline end
  fs.write(dir .. "/.prova/baselines/default.json",
    json.encode({ schema = 1, metrics = { ["demo.size"] = metric } }))
  fs.write(dir .. "/proofs/gate_test.lua",
    'prova.test("ratchet", function(t) measure.ratchet(t, "demo.size", ' .. value .. ') end)\n')
  return dir
end

prova.test("paydown passes while still above the goal and within time (or no deadline)", function(t)
  local dir = workspace(t, 80, nil, 90) -- 90 <= ceiling 100, above goal 80, no deadline
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
end)

prova.test("a met goal graduates: reaching it fails, demanding the goal be locked in and retired", function(t)
  local dir = workspace(t, 80, nil, 80) -- value == goal
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("reached its paydown goal 80")
end)

prova.test("a past deadline with the goal unmet is a hard failure", function(t)
  local dir = workspace(t, 80, "2020-01-01", 90) -- overdue, still 10 from goal
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("missed its paydown deadline 2020-01-01")
end)

prova.test("a future deadline with the goal unmet still passes (debt not yet due)", function(t)
  local dir = workspace(t, 80, "2999-12-31", 90)
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
end)
