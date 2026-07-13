-- Two tests that both need the same port EXCLUSIVELY. Even under high concurrency the scheduler
-- must not overlap them (writer ⊥ writer): (b) cannot enter until (a) has released the port, so the
-- recorded events stay strictly paired — enter a, exit a, enter b, exit b.
--
-- `record` is injected by the harness (tests/resources.rs). The sleep parks each holder long enough
-- that a broken exclusion would show up as interleaved events.
prova.test("boots on :8080 (a)", { resources = { prova.port(8080) } }, function(t)
  record("enter a")
  prova.sleep(40)
  record("exit a")
  t:expect(1):equals(1)
end)

prova.test("boots on :8080 (b)", { resources = { prova.port(8080) } }, function(t)
  record("enter b")
  prova.sleep(40)
  record("exit b")
  t:expect(1):equals(1)
end)
