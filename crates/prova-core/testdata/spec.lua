-- The `spec` flag — TEST-LEVEL ONLY (docs/plans/api-freeze.md §5, revised). A test either
-- carries the flag (a proof authored ahead of its implementation) or it is a full proof with
-- nothing to indicate. A spec'd test that FAILS is an OPEN SPEC — its own outcome, never a
-- failure. One that PASSES is a FAILURE demanding the flag's removal, so the flag can never
-- outlive its implementation. `requires` still wins over spec.

-- An open spec via a failed assertion → outcome `spec`, not `failed`.
prova.test("open spec via assertion", { spec = "gap-1: subset matcher" }, function(t)
  t:expect(1):equals(2)
end)

-- A raise is an open spec too (calling an unimplemented API raises).
prova.test("open spec via raise", { spec = "gap-2: json.encode" }, function(t)
  error("json.encode is not implemented yet")
end)

-- A spec that passes demands the flag's removal — a FAILURE with the remove-it message.
prova.test("honored spec demands flag removal", { spec = "gap-3: already true" }, function(t)
  t:expect(1):equals(1)
end)

-- An unmet `requires` still SKIPS a spec'd test — skip wins over spec (nothing to observe).
prova.test("spec'd but unrunnable skips", { spec = "gap-4: needs tooling", requires = { "definitely_not_a_real_tool_xyzzy" } }, function(t)
  error("must never run")
end)

-- An unflagged test is an ordinary, line-holding proof.
prova.test("ordinary test passes", function(t)
  t:expect(true):is_true()
end)
