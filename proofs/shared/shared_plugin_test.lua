-- Dogfoods the "shared is a plugin" model: a proof requires the local `shared` plugin and uses both
-- its fixture (by handle) and its helper.
local S = require("shared")

prova.test("the shared plugin provides a fixture, by handle", function(t)
  t:expect(t:use(S.greeting)):equals("hello from the shared plugin")
end)

prova.test("the shared plugin provides a plain helper", function(t)
  t:expect(S.slugify("Hello World")):equals("hello-world")
end)
