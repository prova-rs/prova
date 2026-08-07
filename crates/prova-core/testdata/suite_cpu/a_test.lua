-- Half of a RENDEZVOUS (see b_test.lua and tests/suite.rs). This file publishes its own marker and
-- then waits for the other's, which can only appear if b_test.lua is executing at this same moment
-- on another worker with its own Lua state. That is the "files parallelize" claim asserted
-- directly, rather than inferred from a stopwatch that also measures how busy the machine is.
--
-- The wait is a CPU-BOUND SPIN, not a sleep: the claim is about real work overlapping across
-- workers, and a sleeping thread would demonstrate only that the scheduler interleaves waits.
local dir = assert(os.getenv("PROVA_RENDEZVOUS_DIR"), "PROVA_RENDEZVOUS_DIR must be set by the harness")

prova.test("cpu a", function(t)
  fs.write(dir .. "/a.ready", "")
  -- Bounded so the jobs=1 case FAILS rather than hangs — that failure is the negative control.
  local deadline = os.clock() + 5
  while not fs.exists(dir .. "/b.ready") and os.clock() < deadline do end
  t:expect(fs.exists(dir .. "/b.ready"), "b_test.lua must be running concurrently"):is_true()
end)
