-- Unavailable capability → the unit is SKIPPED, not failed (its body never runs).
prova.test("needs a missing tool", { requires = { "definitely_not_a_real_tool_xyzzy" } }, function(t)
  error("must never run")
end)

-- Available capability (sh is on PATH) → runs normally.
prova.test("needs sh which is present", { requires = { "sh" } }, function(t)
  t:expect(1):equals(1)
end)

prova.test("no requirements runs", function(t)
  t:expect(true):is_true()
end)

-- A native-client capability is available when its feature is compiled into the build (sqlite is the
-- embedded database, still bundled), even though there is no `sqlite` binary on PATH — the unified
-- gate checks the build, not PATH. (Before unification this spuriously skipped, looking for a binary.)
prova.test("needs the sqlite client (compiled in)", { requires = { "sqlite" } }, function(t)
  t:expect(1):equals(1)
end)

-- A skipped-by-requires unit cascades to its dependents (skip, not fail).
local gated = prova.test("gated on a missing tool", { requires = { "definitely_not_a_real_tool_xyzzy" } }, function(t)
  error("must never run")
end)
prova.test("depends on the gated unit", { depends_on = { gated } }, function(t)
  error("must never run — upstream was skipped")
end)
