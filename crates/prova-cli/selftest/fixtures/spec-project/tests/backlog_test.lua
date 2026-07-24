-- A package mid-burndown: one open spec (red by definition) + one finished proof. The MCP
-- selftest drives the spec loop against this fixture via the `package` parameter.
prova.test("future feature", { spec = "not built yet" }, function(t)
  t:expect(1):equals(2)
end)

prova.test("shipped feature", function(t)
  t:expect(1):equals(1)
end)
