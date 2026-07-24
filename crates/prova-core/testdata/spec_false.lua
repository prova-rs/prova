-- `spec = false` is not a thing: a test without a spec flag is already a full proof. The value
-- is rejected with the fix, not silently accepted.
prova.test("no such thing as spec = false", { spec = false }, function(t)
  t:expect(1):equals(1)
end)
