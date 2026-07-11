prova.test("beta one", function(t) t:expect("ok"):equals("ok") end)
prova.test("beta skips", function(t) t:skip("not today") end)
