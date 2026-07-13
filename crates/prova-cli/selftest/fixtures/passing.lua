-- Inner fixture: an all-green suite the self-tests run `prova` against.
prova.test("adds numbers", function(t) t:expect(1 + 1):equals(2) end)
prova.test("compares strings", function(t) t:expect("ok"):equals("ok") end)
