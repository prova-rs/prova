-- The `promises` flag — TEST-LEVEL ONLY. A test either carries the flag (a proof authored ahead of
-- its implementation) or it is a full proof with nothing to indicate. A promised test that FAILS is
-- an OPEN PROMISE — its own outcome, never a failure. One that PASSES is a FAILURE demanding the
-- flag graduate to `proves`, so the flag can never outlive its implementation. `requires` still
-- wins over an open promise.

-- An open promise via a failed assertion → its own outcome, not `failed`.
prova.test("open promise via assertion", { promises = "gap-1: subset matcher" }, function(t)
  t:expect(1):equals(2)
end)

-- A raise is an open promise too (calling an unimplemented API raises).
prova.test("open promise via raise", { promises = "gap-2: json.encode" }, function(t)
  error("json.encode is not implemented yet")
end)

-- A promise that passes demands graduation — a FAILURE with the graduate-it message.
prova.test("honored promise demands graduation", { promises = "gap-3: already true" }, function(t)
  t:expect(1):equals(1)
end)

-- An unmet `requires` still SKIPS a promised test — skip wins (nothing to observe).
prova.test("promised but unrunnable skips", { promises = "gap-4: needs tooling", requires = { "definitely_not_a_real_tool_xyzzy" } }, function(t)
  error("must never run")
end)

-- An unflagged test is an ordinary, line-holding proof.
prova.test("ordinary test passes", function(t)
  t:expect(true):is_true()
end)
