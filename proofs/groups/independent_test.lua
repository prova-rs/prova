-- Dogfoods groups: a bag of independent tests (no shared state, order-agnostic, parallelizable).
prova.group("string ops", function(g)
  g:test("upper", function(t) t:expect(("ab"):upper()):equals("AB") end)
  g:test("len",   function(t) t:expect(#"abc"):equals(3) end)
end)
