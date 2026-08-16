--- `json` — a tech-first module: decode AND encode together, with the fidelity sentinels.

prova.test("json.decode parses a document", function(t)
  local v = json.decode('{"a": 1, "xs": [1, 2, 3]}')
  t:expect(v.a):equals(1)
  t:expect(v.xs):has_length(3)
end)

prova.test("json.encode emits a document", function(t)
  t:expect(json.encode({ a = 1 })):equals('{"a":1}')
end)

prova.test("encode and decode round-trip", function(t)
  local v = { name = "prova", ports = { 80, 443 }, nested = { deep = true } }
  t:expect(json.decode(json.encode(v))):equals(v)
end)

prova.test("decode maps null to nil (ergonomic default)", function(t)
  t:expect(json.decode('{"x": null}').x):is_nil()
end)

prova.test("json.null encodes an explicit null", function(t)
  t:expect(json.encode({ x = json.null })):equals('{"x":null}')
end)

prova.test("an empty table encodes as an object; json.array forces a list", function(t)
  t:expect(json.encode({})):equals("{}")
  t:expect(json.encode(json.array{})):equals("[]")
end)

prova.test("prova.parse.json is removed — the clean break to tech-first modules", function(t)
  t:expect(prova.parse.json):is_nil() ---@diagnostic disable-line: undefined-field
end)

prova.test("a decoded array is still an array when it goes back out", {
  covers = "docs/design/agent-ergonomics.md#a-list-verb-returns-a-list",
  proves = "decode used to produce a bare table for `[]`, indistinguishable from a decoded empty OBJECT, so re-encoding turned every empty list into `{}` — a data-shape change at a boundary, silent, and rejected by plenty of APIs that treat the two as different requests. Found while paying down unit coverage on the fidelity layer, which is the kind of defect that exercise is FOR",
}, function(t)
  -- The shapes that can lose their identity, and the nesting where a per-value fix would miss.
  for _, src in ipairs({
    '{"items":[]}',
    '{"items":[1,2]}',
    '{"items":{}}',
    '[[],{},[[]]]',
    '{"a":{"b":[]},"c":[[],[]]}',
  }) do
    t:expect(json.encode(json.decode(src)), src .. " survives the round trip"):equals(src)
  end
end)

prova.test("the array marker is invisible to everything except re-encoding", {
  proves = "the fix stamps a metatable on decoded arrays, and a marker that changed how tables COMPARE or ITERATE would be a worse bug than the one it fixed — every proof that asserts a decoded response against a literal depends on this being transparent",
}, function(t)
  local decoded = json.decode('{"ports":[8080,9090]}')

  t:expect(decoded.ports, "compares equal to a plain literal"):equals({ 8080, 9090 })
  t:expect(#decoded.ports, "length is unchanged"):equals(2)
  t:expect(decoded.ports[1], "indexing is unchanged"):equals(8080)

  local seen = 0
  for _ in ipairs(decoded.ports) do seen = seen + 1 end
  t:expect(seen, "ipairs is unchanged"):equals(2)

  -- …and a structural subset match, the other way proofs read a payload.
  t:expect(decoded, "matches a shape"):matches({ ports = { 8080, 9090 } })
end)
