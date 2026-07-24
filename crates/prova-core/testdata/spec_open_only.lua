-- A suite whose only red is open specs: the run SUCCEEDS (CI stays green while the spec is
-- authored ahead of implementation) — unless `--strict-specs` (the implementing agent's driver
-- mode) turns open specs into real failures.
prova.test("an open spec", { spec = "not yet implemented" }, function(t)
  t:expect(1):equals(2)
end)

prova.test("an ordinary passing test", function(t)
  t:expect(true):is_true()
end)
