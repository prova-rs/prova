-- The `spec` flag: a proof authored ahead of its implementation (docs/plans/api-freeze.md §5).
-- A spec'd test that FAILS is an OPEN SPEC — its own outcome, never a failure. One that PASSES is
-- a FAILURE demanding graduation (`spec = false`), so the flag can never linger past its
-- implementation. Graduated tests are ordinary tests again; `requires` still wins over spec.

-- An open spec via a failed assertion → outcome `spec`, not `failed`.
prova.test("open spec via assertion", { spec = "gap-1: subset matcher" }, function(t)
  t:expect(1):equals(2)
end)

-- A raise is an open spec too (calling an unimplemented API raises).
prova.test("open spec via raise", { spec = true }, function(t)
  error("json.encode is not implemented yet")
end)

-- A spec that passes demands graduation — a FAILURE with the graduate-me message.
prova.test("honored spec demands graduation", { spec = true }, function(t)
  t:expect(1):equals(1)
end)

-- A group-level flag inherits to every leaf below; `spec = false` graduates one back to ordinary.
prova.group("formats", { spec = "api-freeze §1" }, function(g)
  g:test("open under the group flag", function(t)
    t:expect("todo"):equals("done")
  end)
  g:test("graduated, holds the line", { spec = false }, function(t)
    t:expect(1):equals(1)
  end)
end)

-- An unmet `requires` still SKIPS a spec'd test — skip wins over spec (nothing to observe).
prova.test("spec'd but unrunnable skips", { spec = true, requires = { "definitely_not_a_real_tool_xyzzy" } }, function(t)
  error("must never run")
end)
