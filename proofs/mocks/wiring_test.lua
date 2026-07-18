-- End-to-end dogfood of the canonical layout: this proof lives in proofs/mocks/, requires a module
-- from proofs/shared/ (require rooted at proofs/), and drives http.mock (the facet we shipped).
local F = require("shared.fixtures")

prova.test("the layout wires up: require a shared fixture, then drive a mock", function(t)
  t:expect(t:use(F.greeting)):equals("hello from shared/")

  local m = http.mock(t)
  m:on{ path = "/ping" }:reply{ status = 200, body = "pong" }
  t:expect(http.get(m.url .. "/ping").body):equals("pong")
end)
