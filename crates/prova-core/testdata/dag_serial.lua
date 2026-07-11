-- A 3-deep dependency chain, each link sleeping 40ms. Under high concurrency the scheduler still
-- must serialize a → b → c (an edge orders and gates), so wall-clock ≈ 3×40ms, not 40ms.
local a = prova.test("a", function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)

local b = prova.test("b", { depends_on = { a } }, function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)

prova.test("c", { depends_on = { b } }, function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)
