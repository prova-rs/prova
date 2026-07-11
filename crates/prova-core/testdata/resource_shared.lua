-- Two tests that share the same resource as READERS. They may run at the same time (reader ∥
-- reader), so under concurrency wall-clock ≈ 40ms, not 80ms.
prova.test("reads shared db (a)", { resources = { prova.shared("db") } }, function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)

prova.test("reads shared db (b)", { resources = { prova.shared("db") } }, function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)
