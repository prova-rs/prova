local G = require("greet")
prova.test("resolves a local plugin under .prova/plugins", function(t)
  t:expect(t:use(G.greeter)):equals("from a local plugin")
end)
