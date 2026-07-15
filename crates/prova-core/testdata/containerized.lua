-- prova.containerized shape checks (no docker needed — we don't call .container here).

prova.test("builds a grammar-conformant namespace", function(t)
  local ns = prova.containerized{
    name = "demo", image = "demo", tag = "1", port = 1234,
    url = function(hp) return "demo://127.0.0.1:" .. hp end,
    client = function(url) return { url = url } end,
  }
  t:expect(type(ns.container)):equals("function")
  t:expect(type(ns.client)):equals("function")
end)

prova.test("client is the factory the spec supplied (identity)", function(t)
  local factory = function(url) return { url = url } end
  local ns = prova.containerized{ image = "x", port = 1, url = function(hp) return hp end, client = factory }
  t:expect(ns.client == factory):is_true()
end)

prova.test("client is absent when the spec omits it (black-box provisioning)", function(t)
  local ns = prova.containerized{ image = "x", port = 1, url = function(hp) return hp end }
  t:expect(ns.client == nil):is_true()
  t:expect(type(ns.container)):equals("function")
end)

prova.test("requires image and url", function(t)
  t:expect(pcall(prova.containerized, { name = "x", port = 1 })):is_false()             -- no url
  t:expect(pcall(prova.containerized, { url = function() end })):is_false()             -- no image
end)

prova.test("requires a port", function(t)
  t:expect(pcall(prova.containerized, { image = "x", url = function() end })):is_false()
end)

prova.test("accepts a fixed-port ports entry and an extra hook", function(t)
  -- `{ container, host }` fixed-port form (primary resolves to the container port) + an `extra` hook.
  local ns = prova.containerized{
    name = "fx", image = "x",
    ports = { { container = 9092, host = 9092 } },
    url = function(hp) return "x://" .. hp end,
    extra = function() return { token = "t" } end,
  }
  t:expect(type(ns.container)):equals("function")
end)
