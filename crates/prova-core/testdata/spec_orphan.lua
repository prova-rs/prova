-- `spec = false` graduates a test OUT of an enclosing spec flag. With no flag to graduate from,
-- the marker is dead weight — a validation error, so stale markers can't linger after a flag is
-- removed.
prova.test("orphan graduation", { spec = false }, function(t)
  t:expect(1):equals(1)
end)
