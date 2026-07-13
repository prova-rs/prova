-- test_each: one test per case, with `{placeholder}` names filled from the case, the case delivered
-- both as the body's 2nd argument and as `t.case`. Covers the top-level and GroupBuilder variants.

-- Top-level: 3 cases → 3 tests named "squares 2", "squares 3", "squares 4".
prova.test_each("squares {n}", {
  { n = 2, want = 4 },
  { n = 3, want = 9 },
  { n = 4, want = 16 },
}, function(t, case)
  t:expect(case.n * case.n):equals(case.want)
end)

-- `t.case` is the same table as the 2nd argument.
prova.test_each("t.case matches arg for {label}", {
  { label = "a", v = 1 },
  { label = "b", v = 2 },
}, function(t, case)
  t:expect(t.case.v):equals(case.v)
  t:expect(t.case.label):equals(case.label)
end)

-- Inside a group.
prova.group("parametrized", function(g)
  g:test_each("doubles {n}", {
    { n = 5, want = 10 },
    { n = 6, want = 12 },
  }, function(t, case)
    t:expect(case.n * 2):equals(case.want)
  end)
end)

-- An ordinary test still works with a single `t` parameter (ignores the trailing nil arg).
prova.test("plain test unaffected", function(t)
  t:expect(t.case):is_nil()
end)
