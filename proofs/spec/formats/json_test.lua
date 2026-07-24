--- `json` — a tech-first module: decode AND encode together, with the fidelity sentinels.

prova.test("json.decode parses a document", { spec = "api-freeze §1 - json module" }, function(t)
  local v = json.decode('{"a": 1, "xs": [1, 2, 3]}')
  t:expect(v.a):equals(1)
  t:expect(v.xs):has_length(3)
end)

prova.test("json.encode emits a document", { spec = "api-freeze §1 - json module" }, function(t)
  t:expect(json.encode({ a = 1 })):equals('{"a":1}')
end)

prova.test("encode and decode round-trip", { spec = "api-freeze §1 - json module" }, function(t)
  local v = { name = "prova", ports = { 80, 443 }, nested = { deep = true } }
  t:expect(json.decode(json.encode(v))):equals(v)
end)

prova.test("decode maps null to nil (ergonomic default)", { spec = "api-freeze §1 - json module" }, function(t)
  t:expect(json.decode('{"x": null}').x):is_nil()
end)

prova.test("json.null encodes an explicit null", { spec = "api-freeze §1 - json module" }, function(t)
  t:expect(json.encode({ x = json.null })):equals('{"x":null}')
end)

prova.test("an empty table encodes as an object; json.array forces a list", { spec = "api-freeze §1 - json module" }, function(t)
  t:expect(json.encode({})):equals("{}")
  t:expect(json.encode(json.array{})):equals("[]")
end)

prova.test("prova.parse.json is removed — the clean break to tech-first modules", { spec = "api-freeze §1 - json module" }, function(t)
  t:expect(prova.parse.json):is_nil()
end)
