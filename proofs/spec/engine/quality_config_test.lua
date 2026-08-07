-- Spec: the [quality] section resolves and is surfaced to authors as `prova.quality` (posture +
-- thresholds) — the dials a language pack reads instead of hardcoding. Language-agnostic. Black-box
-- via prova.bin; the nested proof asserts what it sees, and a negative control proves the value is
-- really read (not vacuously passing).

local function workspace(t, manifest_quality, body)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n' .. (manifest_quality or ""))
  fs.write(dir .. "/proofs/q_test.lua", body)
  return dir
end

prova.test("prova.quality reflects the [quality] posture and thresholds", function(t)
  local dir = workspace(t, '[quality]\nposture = "observe"\nmax_file_lines = 1234\n', [[
    prova.test("read", function(t)
      t:expect(prova.quality.posture):equals("observe")
      t:expect(prova.quality.max_file_lines):equals(1234)
    end)
  ]])
  t:expect(shell.run({ prova.bin }, { cwd = dir, merge_stderr = true }).code, "expected green"):equals(0)
end)

prova.test("prova.quality defaults to enforce with no threshold when [quality] is absent", function(t)
  local dir = workspace(t, nil, [[
    prova.test("read", function(t)
      t:expect(prova.quality.posture):equals("enforce")
      t:expect(prova.quality.max_file_lines):is_nil()
    end)
  ]])
  t:expect(shell.run({ prova.bin }, { cwd = dir, merge_stderr = true }).code):equals(0)
end)

prova.test("negative control: the config is really read (a wrong expectation fails)", function(t)
  local dir = workspace(t, '[quality]\nposture = "observe"\n', [[
    prova.test("read", function(t)
      t:expect(prova.quality.posture):equals("enforce") -- WRONG on purpose; manifest says observe
    end)
  ]])
  t:expect(shell.run({ prova.bin }, { cwd = dir, merge_stderr = true }).code):never():equals(0)
end)

prova.test("--observe overrides the manifest posture for the run", function(t)
  local dir = workspace(t, '[quality]\nposture = "enforce"\n', [[
    prova.test("read", function(t) t:expect(prova.quality.posture):equals("observe") end)
  ]])
  t:expect(shell.run({ prova.bin, "--observe" }, { cwd = dir, merge_stderr = true }).code):equals(0)
end)
