-- Two tests that both need the same port EXCLUSIVELY. Even under high concurrency the scheduler
-- must not overlap them (writer ⊥ writer), so wall-clock ≈ 2×40ms.
prova.test("boots on :8080 (a)", { resources = { prova.port(8080) } }, function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)

prova.test("boots on :8080 (b)", { resources = { prova.port(8080) } }, function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)
