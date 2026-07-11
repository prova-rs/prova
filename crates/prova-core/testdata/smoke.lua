-- POC smoke test: only the implemented subset (prova.test / prova.group / t:expect).
-- `prova` is an injected global — no require.

prova.test("arithmetic and strings", function(t)
  t:expect(1 + 1):equals(2)
  t:expect("hello world"):contains("world")
  t:expect(nil):is_nil()
  t:expect(3):never():equals(4)
end)

prova.group("a group", function(g)
  g:test("boolean truthiness", function(t)
    t:expect(true):is_true()
    t:expect(0):is_truthy() -- 0 is truthy in Lua
  end)

  g:test("intentional failure", function(t)
    t:expect(1, "the answer"):equals(2)
  end)

  g:test("explicit skip", function(t)
    t:skip("not ready yet")
  end)
end)
