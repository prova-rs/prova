prova.test("always passes", function(t) t:expect(1):equals(1) end)
prova.test("always fails", function(t) t:expect(1, "deliberate red"):equals(2) end)
prova.test("tagged slow", { tags = { "slow" } }, function(t) t:expect(3):equals(3) end)
