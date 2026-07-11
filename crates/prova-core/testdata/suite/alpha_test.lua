prova.test("alpha one", function(t) t:expect(1):equals(1) end)
prova.test("alpha two", function(t) t:expect(2):equals(2) end)
prova.test("alpha fails", function(t) t:expect(1):equals(2) end)
