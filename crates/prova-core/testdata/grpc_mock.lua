-- grpc.mock — the `mock` facet on the gRPC namespace.
--
-- The bar is set by the last line of every test here: **the driving client is `grpc.client`**, the
-- real one, unmodified. It learns the mock's schema the same way it learns a real server's — over
-- reflection, with no `.proto` on the client side. If the unmodified client cannot tell the mock
-- from a server, it is a server. That is the proof; anything less tests a bespoke loopback.
--
-- Note the asymmetry this facet has to solve, and that Phase A did not: the client is schema-free
-- because it learns the schema FROM the server. A mock *is* the server, so that trick does not
-- invert — it has to be told. `proto` is the source here; a descriptor set and reflection-harvesting
-- from a live service are the other two (see docs/plans/mocks.md §6).
--
-- The proto is written at runtime rather than checked in beside this file: a testdata proof runs
-- with no manifest, so `prova.root` is nil and there is no anchor to resolve a sibling path against.
-- Writing it here also keeps the schema visible next to the assertions that depend on it.

local proto = prova.fixture("proto", Scope.File, function(ctx)
  local path = ctx:tempdir() .. "/pricing.proto"
  fs.write(path, [[
syntax = "proto3";
package pricing;

service Pricing {
  rpc GetPrice (PriceRequest) returns (PriceReply);
  rpc Health (HealthRequest) returns (HealthReply);
}

message PriceRequest {
  string sku = 1;
  int32 qty = 2;
}
message PriceReply {
  string sku = 1;
  int32 cents = 2;
}
message HealthRequest {}
message HealthReply { bool ok = 1; }
]])
  return path
end)

prova.test("the unmodified grpc client drives the mock, via reflection", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method = "pricing.Pricing/GetPrice" }
   :reply{ response = { sku = "A1", cents = 999 } }

  local c = grpc.client(m.url) -- reflection: no .proto on this side
  local res = c:call("pricing.Pricing/GetPrice", { sku = "A1" })
  t:expect(res.sku):equals("A1")
  t:expect(res.cents):equals(999)
end)

-- THE runtime proof, gRPC edition: `cents` is computed from the request, in Lua, while this
-- coroutine is parked inside c:call. It cannot be precomputed.
prova.test("a Lua handler answers an RPC while the test is suspended", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method = "pricing.Pricing/GetPrice" }:reply(function(req)
    return { response = { sku = req.request.sku, cents = 100 * req.request.qty } }
  end)

  local c = grpc.client(m.url)
  local res = c:call("pricing.Pricing/GetPrice", { sku = "Z9", qty = 3 })
  t:expect(res.sku):equals("Z9")
  t:expect(res.cents):equals(300)
end)

prova.test("a handler closes over test-local state", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  local seen = {}
  m:on{ method = "pricing.Pricing/GetPrice" }:reply(function(req)
    table.insert(seen, req.request.sku)
    return { response = { sku = req.request.sku, cents = #seen } }
  end)

  local c = grpc.client(m.url)
  t:expect(c:call("pricing.Pricing/GetPrice", { sku = "A" }).cents):equals(1)
  t:expect(c:call("pricing.Pricing/GetPrice", { sku = "B" }).cents):equals(2)
  t:expect(seen):equals({ "A", "B" }) -- the upvalue the test can see was appended to by the handler
end)

-- The reply vocabulary mirrors `call_status`'s report exactly: what the client tells you a server
-- answered is what you write to make the mock answer it. One spelling, both directions.
prova.test("a stub can answer a gRPC status instead of a message", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method = "pricing.Pricing/GetPrice" }
   :reply{ code = "NotFound", message = "unknown sku" }

  local c = grpc.client(m.url)
  local r = c:call_status("pricing.Pricing/GetPrice", { sku = "nope" })
  t:expect(r.ok):is_false()
  t:expect(r.code):equals("NotFound")
  t:expect(r.message):equals("unknown sku")
end)

prova.test("a handler can answer a status too", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method = "pricing.Pricing/GetPrice" }:reply(function(req)
    if req.request.qty > 10 then
      return { code = "ResourceExhausted", message = "too many" }
    end
    return { response = { sku = req.request.sku, cents = 1 } }
  end)

  local c = grpc.client(m.url)
  t:expect(c:call_status("pricing.Pricing/GetPrice", { sku = "A", qty = 1 }).ok):is_true()
  local r = c:call_status("pricing.Pricing/GetPrice", { sku = "A", qty = 99 })
  t:expect(r.ok):is_false()
  t:expect(r.code):equals("ResourceExhausted")
end)

prova.test("records the RPCs it was asked", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method = "pricing.Pricing/GetPrice" }:reply{ response = { sku = "x", cents = 1 } }
  m:on{ method = "pricing.Pricing/Health" }:reply{ response = { ok = true } }

  local c = grpc.client(m.url)
  c:call("pricing.Pricing/GetPrice", { sku = "A1", qty = 2 })
  c:call("pricing.Pricing/Health", {})
  c:call("pricing.Pricing/GetPrice", { sku = "B2" })

  t:expect(m:received()):has_length(3)

  local calls = m:received{ method = "pricing.Pricing/GetPrice" }
  t:expect(calls):has_length(2)
  t:expect(calls[1].request.sku):equals("A1")
  t:expect(calls[1].request.qty):equals(2)
  t:expect(calls[1].code):equals("Ok") -- what we answered
  t:expect(calls[2].request.sku):equals("B2")
end)

prova.test("an unstubbed method is Unimplemented, and is still recorded", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })

  local c = grpc.client(m.url)
  local r = c:call_status("pricing.Pricing/Health", {})
  t:expect(r.ok):is_false()
  t:expect(r.code):equals("Unimplemented")

  t:expect(m:received()):has_length(1)
  t:expect(m:received()[1].matched):is_false()
end)

-- What this proves is reflection *fidelity*, not the server's unknown-method path: the client refuses
-- `Nope` before a byte leaves the process, because the schema it learned from the mock genuinely
-- doesn't define it. Worth stating plainly — the server's own "not in the schema" branch
-- (Unimplemented, distinct from "defined but unstubbed") is unreachable from `grpc.client` by
-- construction, since the mock's reflection and its dispatch are built from the same descriptor
-- bytes. That branch exists for the case that matters in the field and that nothing here covers: a
-- **real SUT** calling a method the mock's proto omits.
prova.test("the schema the mock advertises is the schema it has", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  local c = grpc.client(m.url)
  local ok = pcall(function() c:call("pricing.Pricing/Nope", {}) end)
  t:expect(ok):is_false()
end)

prova.test("first matching stub wins, and method_matches takes a Lua pattern", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method_matches = "^pricing%.Pricing/" }:reply{ response = { sku = "wild", cents = 1 } }
  m:on{ method = "pricing.Pricing/GetPrice" }:reply{ response = { sku = "exact", cents = 2 } }

  local c = grpc.client(m.url)
  t:expect(c:call("pricing.Pricing/GetPrice", { sku = "A" }).sku):equals("wild")
end)

prova.test("is grammar-shaped: url, host, port", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  t:expect(m.host):equals("127.0.0.1")
  t:expect(m.port):gt(1024)
  t:expect(m.url):equals("http://127.0.0.1:" .. m.port)
end)

-- `allow_handler_errors` because the error path IS the subject here; strictness is the default.
prova.test("a raising handler answers Internal and records the error", function(t)
  local m = grpc.mock(t, { proto = t:use(proto), allow_handler_errors = true })
  m:on{ method = "pricing.Pricing/Health" }:reply(function() error("handler blew up") end)

  local c = grpc.client(m.url)
  local r = c:call_status("pricing.Pricing/Health", {})
  t:expect(r.ok):is_false()
  t:expect(r.code):equals("Internal")
  t:expect(m:received()[1].error):contains("handler blew up")
end)

prova.test("grpc.wait_for sees the mock, like any server", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  grpc.wait_for(m.url, { timeout = "5s" })
end)
