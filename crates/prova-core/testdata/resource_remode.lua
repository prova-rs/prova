-- The two constructors are modes, not kinds, so either can re-mode what the other made.
--
-- `prova.reads(prova.port(8080))` widens a port — exclusive on its own — into a concurrent hold, so
-- these two overlap where `resource_exclusive.lua`'s pair (same token, writer mode) serializes. That
-- pair of files is the whole point: identical token, opposite scheduling, decided only by the mode.
--
-- And `prova.writes` on a bare token serializes, the same way a bare string does. `record` is
-- injected by the harness (tests/resources.rs).
-- The readers RENDEZVOUS rather than racing a clock. Each records its entry, then waits for the
-- other; neither can record an exit until both have entered, so `ra`/`rb` interleaving is a
-- CONSEQUENCE of concurrency rather than a 40ms window the scheduler has to hit. The old shape
-- (sleep, then compare timestamps) failed twice on a loaded box with nothing wrong, and would have
-- passed just as happily if the reader lock had started serializing — timing luck in both
-- directions. If these ever stop running concurrently the barrier times out and says so.
prova.test("port as reader (a)", { resources = { prova.reads(prova.port(8080)) } }, function(t)
  record("enter ra")
  prova.barrier("remode-readers", 2, { timeout = "20s" })
  record("exit ra")
  t:expect(1):equals(1)
end)

prova.test("port as reader (b)", { resources = { prova.reads(prova.port(8080)) } }, function(t)
  record("enter rb")
  prova.barrier("remode-readers", 2, { timeout = "20s" })
  record("exit rb")
  t:expect(1):equals(1)
end)

-- Writers keep the sleep on purpose. They must SERIALIZE, so a barrier between them would
-- deadlock by construction — and "did not overlap" is the one direction load cannot falsify.
prova.test("named writer (a)", { resources = { prova.writes("db") } }, function(t)
  record("enter wa")
  prova.sleep(40)
  record("exit wa")
  t:expect(1):equals(1)
end)

prova.test("named writer (b)", { resources = { prova.writes("db") } }, function(t)
  record("enter wb")
  prova.sleep(40)
  record("exit wb")
  t:expect(1):equals(1)
end)
