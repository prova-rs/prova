-- Dogfoods the fixture model: Scope.Test rebuilds per test; ctx:defer runs teardown LIFO.
local counter = prova.fixture("counter", Scope.Test, function(ctx)
  local n = { value = 1 }
  ctx:defer(function() n.value = 0 end)   -- teardown (observable only within scope)
  return n
end)

prova.test("a Scope.Test fixture is built fresh", function(t)
  t:expect(t:use(counter).value):equals(1)
end)

prova.test("...and rebuilt for the next test, not carried over", function(t)
  t:expect(t:use(counter).value):equals(1)   -- 1 again, not mutated from the prior test
end)
