-- The retired spellings, held to the SEMANTICS of their successors — not merely to "does not error".
-- `prova.shared` == `prova.reads` (readers overlap) and `prova.resource` == `prova.writes` (writers
-- serialize). Two independent tokens so the two claims can't contend with each other.
--
-- This file is deliberately the only place in the tree that still calls the old words: they are
-- unadvertised, so nothing else should teach them, but a suite written before the rename must keep
-- running. `record` is injected by the harness (tests/resources.rs).
--
-- The readers RENDEZVOUS instead of sleeping: neither exits until both have entered, so the
-- interleave proves concurrency rather than racing a 40ms window (see resource_remode.lua and
-- crate::barrier). Writers keep their sleeps — they must serialize, so a barrier would deadlock,
-- and "did not overlap" is the direction load cannot falsify.
prova.test("legacy shared reader (a)", { resources = { prova.shared("db") } }, function(t)
  record("enter ra")
  prova.barrier("retired-readers", 2, { timeout = "20s" })
  record("exit ra")
  t:expect(1):equals(1)
end)

prova.test("legacy shared reader (b)", { resources = { prova.shared("db") } }, function(t)
  record("enter rb")
  prova.barrier("retired-readers", 2, { timeout = "20s" })
  record("exit rb")
  t:expect(1):equals(1)
end)

prova.test("legacy exclusive writer (a)", { resources = { prova.resource("acct") } }, function(t)
  record("enter wa")
  prova.sleep(40)
  record("exit wa")
  t:expect(1):equals(1)
end)

prova.test("legacy exclusive writer (b)", { resources = { prova.resource("acct") } }, function(t)
  record("enter wb")
  prova.sleep(40)
  record("exit wb")
  t:expect(1):equals(1)
end)
