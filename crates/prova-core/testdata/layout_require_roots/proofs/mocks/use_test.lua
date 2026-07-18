local T = require("shared.thing")
prova.test("require resolves against a configured root, not the home", function(t)
  t:expect(T.value):equals(7)
end)
