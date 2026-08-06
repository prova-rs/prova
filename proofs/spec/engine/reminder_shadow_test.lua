-- Spec: a reminder's `when` condition can read the run's measurements (docs/design/verifiers.md).
-- This is the "one claim, two surfaces by tense" payoff — the same scalar a ratchet gates on in the
-- past (proof) is what a heed reminder watches in the future (nudge). Black-box via prova.bin;
-- --heed turns a DUE reminder into a non-zero exit.

local MANIFEST = '[run]\nproofs = ["proofs"]\n'

-- A workspace whose proof records demo.size and declares a reminder that fires when it exceeds 500.
local function workspace(t, recorded_value)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", MANIFEST)
  fs.write(dir .. "/proofs/shadow_test.lua", table.concat({
    'prova.test("record the size", function(t)',
    '  measure.record("demo.size", ' .. recorded_value .. ', { direction = "lower_is_better" })',
    'end)',
    'prova.remind("size-watch", {',
    '  when = function(a)',
    '    local v = a.measurements["demo.size"] or 0',
    '    if v > 500 then return "demo.size at " .. v .. "/500" end',
    '  end,',
    '}, "split the file before it grows further")',
  }, "\n") .. "\n")
  return dir
end

prova.test("a reminder condition reads a run measurement and fires DUE above the threshold", function(t)
  local dir = workspace(t, 600)
  local r = shell.run({ prova.bin, "--heed" }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0) -- DUE + --heed => red
  t:expect(r.stdout):contains("demo.size at 600") -- the condition read the measurement (600/500)
end)

prova.test("the same reminder stays WATCHING (green) below the threshold", function(t)
  local dir = workspace(t, 400)
  local r = shell.run({ prova.bin, "--heed" }, { cwd = dir, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0) -- watching => green even under --heed
end)
