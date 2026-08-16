--- `prova.barrier` — proving two units were in flight AT ONCE
--- (docs/design/agent-ergonomics.md#every-wait-is-bounded).
---
--- Concurrency had no primitive before this. `prova.sleep` measures timing luck in BOTH
--- directions: it fails when a loaded host does not schedule the second unit inside the window,
--- and it passes when a serialized system happens to overlap anyway. The locks serialize, which is
--- the opposite. So "my service handles two concurrent requests" was written with a stopwatch.
---
--- A barrier turns the observation into a precondition: reaching the far side IS the proof, with
--- no window to get lucky with.

local scaffold = require("scaffold")

local function ran(t, name, proof, flags)
  local proj = scaffold.package(t, { name = name, proofs = { ["b_test.lua"] = proof } })
  local argv = { prova.bin }
  for _, f in ipairs(flags or {}) do argv[#argv + 1] = f end
  return shell.run(argv, { cwd = proj, merge_stderr = true, timeout = "90s" })
end

local PAIR = [[
prova.test("a", function(t) prova.barrier("pair", 2, { timeout = "20s" }) t:expect(1):equals(1) end)
prova.test("b", function(t) prova.barrier("pair", 2, { timeout = "20s" }) t:expect(1):equals(1) end)
]]

prova.test("two units that CAN run at once pass the barrier", {
  covers = "docs/design/agent-ergonomics.md#every-wait-is-bounded",
  proves = "the whole point: neither reaches the far side until both are inside, so a green run is itself the evidence they overlapped — there is no timestamp to compare and no window to have been lucky in",
}, function(t)
  local r = ran(t, "concurrent", PAIR, { "-j", "4" })
  t:expect(r.code, "both arrived: " .. r.stdout):equals(0)
  t:expect(r.stdout, "…and both passed"):contains("2 passed")
end)

prova.test("a barrier that cannot be satisfied FAILS, and says why", {
  covers = "docs/design/agent-ergonomics.md#every-wait-is-bounded",
  proves = "a hang reports nothing — no verdict, no seam, no exit code until an outer timeout kills it. The bound is what converts 'the suite stopped' into a finding that names how many of how many arrived",
}, function(t)
  -- Serialized: the second unit cannot start until the first returns, so the first waits alone.
  local r = ran(t, "serialized", [[
prova.test("a", function(t) prova.barrier("solo", 2, { timeout = "2s" }) t:expect(1):equals(1) end)
]], { "-j", "1" })
  t:expect(r.code, "the run fails rather than hanging"):never():equals(0)
  t:expect(r.stdout, "…naming the shortfall"):contains("1 arrived")
  t:expect(r.stdout, "…and the likeliest cause first"):contains("not SELECTED")
  t:expect(r.stdout, "…with serialization named too"):contains("-j 1")
end)

prova.test("a completed barrier leaves nothing that would satisfy the next one", {
  covers = "docs/design/agent-ergonomics.md#every-wait-is-bounded",
  proves = "this primitive nearly shipped with the exact defect it exists to prevent: arrival state outlived its run, so a second run saw a satisfied barrier and sailed through with nothing overlapping — a vacuous pass, from the thing built to make vacuous passes impossible",
}, function(t)
  local proj = scaffold.package(t, { name = "repeat", proofs = { ["b_test.lua"] = PAIR } })
  for attempt = 1, 2 do
    local r = shell.run({ prova.bin, "-j", "4" },
      { cwd = proj, merge_stderr = true, timeout = "90s" })
    t:expect(r.code, "run " .. attempt .. " stands on its own: " .. r.stdout):equals(0)
  end
end)

prova.test("the barrier's own options are closed, like every other module table", {
  covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
  proves = "a typo'd `timeout` on a WAIT is the worst possible silent drop — the call would fall back to the default patience while the author believes they set a different one, and the symptom is a run that is slower or flakier than it reads",
}, function(t)
  local r = ran(t, "typo", [[
prova.test("a", function(t) prova.barrier("x", 1, { timeuot = "1s" }) t:expect(1):equals(1) end)
]])
  t:expect(r.code, "the typo is refused"):never():equals(0)
  t:expect(r.stdout, "…naming the key"):contains("timeuot")
  t:expect(r.stdout, "…and the spelling meant"):contains("timeout")
end)

-- NOTE: that a barrier with NO `timeout` is still bounded (the 30s default) is proven at the unit
-- level in `crate::barrier` rather than here. Asserting it black-box costs a real 30s wait in
-- every sweep, and the property — a defaulted, finite bound — is a constant, not a behavior that
-- can drift between the Rust and what an author sees.
