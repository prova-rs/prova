-- Two tests that share the same resource as READERS. They may run at the same time (reader ∥
-- reader): the scheduler is free to start (b) while (a) is parked in its sleep, so the recorded
-- events interleave — enter a, enter b, exit a, exit b.
--
-- `record` is injected by the harness (tests/resources.rs). The sleep is what creates the yield
-- point at which an overlap becomes observable.
prova.test("reads shared db (a)", { resources = { prova.shared("db") } }, function(t)
  record("enter a")
  prova.sleep(40)
  record("exit a")
  t:expect(1):equals(1)
end)

prova.test("reads shared db (b)", { resources = { prova.shared("db") } }, function(t)
  record("enter b")
  prova.sleep(40)
  record("exit b")
  t:expect(1):equals(1)
end)
