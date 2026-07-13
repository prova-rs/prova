-- Inner fixture: one of each outcome, so the self-tests can assert the tally + exit code.
prova.test("passes", function(t) t:expect(true):is_true() end)
prova.test("fails", function(t) t:expect(1):equals(2) end)
prova.test("skips", function(t) t:skip("not today") end)
