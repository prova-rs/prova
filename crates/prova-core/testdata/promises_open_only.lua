-- A suite whose only red is open promises: the run SUCCEEDS (CI stays green while the contract is
-- authored ahead of implementation) — unless `--due` (the implementing agent's driver mode) turns
-- open promises into real failures.
prova.test("an open promise", { promises = "not yet implemented" }, function(t)
  t:expect(1):equals(2)
end)

prova.test("an ordinary passing test", function(t)
  t:expect(true):is_true()
end)
