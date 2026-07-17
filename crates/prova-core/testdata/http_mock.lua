-- http.mock — an in-process HTTP server you stub, drive, and assert on.
--
-- The load-bearing test is "a Lua handler answers": the handler runs on the *same Lua state* while
-- this test's coroutine is suspended inside http.post. That is the runtime assumption
-- docs/plans/mocks.md §3 rests on, and the reason the mini-language it would otherwise need is not
-- built. If the assumption is wrong this cannot pass by accident — it hangs or raises.
--
-- Readiness is http.mock's contract: the listener is bound before the call returns, so every probe
-- here is a *single attempt* with no prova.retry. Margin would hide exactly what is being proved —
-- the docker_readiness.lua bar, applied to a server we own.

prova.test("serves a declarative stub", function(t)
  local m = http.mock(t)
  m:on{ method = "GET", path = "/v1/price/A1" }
   :reply{ status = 200, json = { sku = "A1", cents = 999 } }

  local res = http.get(m.url .. "/v1/price/A1")
  t:expect(res.status):equals(200)
  t:expect(res:json().cents):equals(999)
  t:expect(res.headers["content-type"]):contains("application/json")
end)

-- THE runtime proof. `saw` and `at` cannot be precomputed: they are derived from the request, in
-- Lua, while this coroutine is parked inside http.post.
prova.test("a Lua handler answers while the test is suspended", function(t)
  local m = http.mock(t)
  m:on{ method = "POST", path = "/echo" }:reply(function(req)
    return { status = 201, json = { saw = req.json.sku, at = req.path, verb = req.method } }
  end)

  local res = http.post(m.url .. "/echo", { json = { sku = "A1" } })
  t:expect(res.status):equals(201)
  local body = res:json()
  t:expect(body.saw):equals("A1")
  t:expect(body.at):equals("/echo")
  t:expect(body.verb):equals("POST")
end)

-- A handler closing over test-local state proves it is genuinely *this* Lua state, not a copy.
prova.test("a handler closes over test-local state", function(t)
  local m = http.mock(t)
  local calls = 0
  m:on{ path = "/count" }:reply(function()
    calls = calls + 1
    return { status = 200, json = { n = calls } }
  end)

  t:expect(http.get(m.url .. "/count"):json().n):equals(1)
  t:expect(http.get(m.url .. "/count"):json().n):equals(2)
  t:expect(calls):equals(2) -- the upvalue the test can see was mutated by the handler
end)

-- A handler that re-enters the mock it belongs to. This is the borrow the implementation must NOT be
-- holding: `handle` resolves the match and clones the reply out before awaiting into Lua, precisely
-- so a handler can read the journal or register a stub mid-request. A comment claiming that is worth
-- nothing — only this is. If the borrow leaks, this panics rather than fails.
prova.test("a handler can re-enter the mock it belongs to", function(t)
  local m = http.mock(t)
  m:on{ path = "/reenter" }:reply(function()
    local seen = #m:received()                                     -- borrows the journal, mid-request
    m:on{ path = "/late" }:reply{ status = 200, body = "late" }    -- mutates the stub list, mid-request
    return { status = 200, json = { seen = seen } }
  end)

  -- A request is journaled after its handler returns, so the in-flight one is not yet visible.
  t:expect(http.get(m.url .. "/reenter"):json().seen):equals(0)
  t:expect(http.get(m.url .. "/late").body):equals("late")
end)

-- ctx:manage calls stop() at scope end regardless of what the test did, so an explicit stop must not
-- turn a passing test into a teardown error.
prova.test("stop() is idempotent", function(t)
  local m = http.mock(t)
  m:stop()
  m:stop()
end)

prova.test("records what it was asked", function(t)
  local m = http.mock(t)
  m:on{ path = "/v1/price/A1" }:reply{ status = 200, json = {} }

  http.get(m.url .. "/v1/price/A1", { headers = { ["X-Idempotency-Key"] = "k1" } })
  http.get(m.url .. "/v1/price/A1")
  http.get(m.url .. "/other")

  t:expect(m:received()):has_length(3)

  local calls = m:received{ path = "/v1/price/A1" }
  t:expect(calls):has_length(2)
  t:expect(calls[1].method):equals("GET")
  t:expect(calls[1].headers["x-idempotency-key"]):equals("k1") -- header names normalize to lowercase
  t:expect(calls[1].status):equals(200)                        -- what we answered
end)

prova.test("records the request body, decoded", function(t)
  local m = http.mock(t)
  m:on{ path = "/orders" }:reply{ status = 201 }

  http.post(m.url .. "/orders", { json = { sku = "A1", qty = 2 } })

  local r = m:received()[1]
  t:expect(r.json.sku):equals("A1")
  t:expect(r.json.qty):equals(2)
  t:expect(r.body):contains("A1") -- the raw bytes are kept too
end)

prova.test("an unmatched request is a 404, and is still recorded", function(t)
  local m = http.mock(t)
  local res = http.get(m.url .. "/nope")
  t:expect(res.status):equals(404)
  t:expect(m:received()):has_length(1)
  t:expect(m:received()[1].matched):is_false()
end)

prova.test("first matching stub wins", function(t)
  local m = http.mock(t)
  m:on{ path_matches = "^/v1/" }:reply{ status = 200, body = "first" }
  m:on{ path = "/v1/x" }:reply{ status = 200, body = "second" }
  t:expect(http.get(m.url .. "/v1/x").body):equals("first")
end)

prova.test("matches on method, and a path-only stub matches any method", function(t)
  local m = http.mock(t)
  m:on{ method = "DELETE", path = "/r" }:reply{ status = 204 }
  m:on{ path = "/r" }:reply{ status = 200 }

  t:expect(http.delete(m.url .. "/r").status):equals(204)
  t:expect(http.get(m.url .. "/r").status):equals(200)
end)

prova.test("query strings are parsed and do not defeat path matching", function(t)
  local m = http.mock(t)
  m:on{ path = "/search" }:reply{ status = 200 }

  http.get(m.url .. "/search?q=widget&limit=2")

  local r = m:received()[1]
  t:expect(r.path):equals("/search")
  t:expect(r.query.q):equals("widget")
  t:expect(r.query.limit):equals("2")
end)

prova.test("a raising handler answers 500 and records the error", function(t)
  local m = http.mock(t)
  m:on{ path = "/boom" }:reply(function() error("handler blew up") end)

  local res = http.get(m.url .. "/boom")
  t:expect(res.status):equals(500)
  t:expect(m:received()[1].error):contains("handler blew up")
end)

-- `route` exists because the alternative is spelling one path twice — `path_matches = "^/orders/"`
-- to match, and `req.path:match("/orders/(.+)$")` to extract — in two languages free to drift.
prova.test("route captures path params", function(t)
  local m = http.mock(t)
  m:on{ method = "GET", route = "/orders/:id" }:reply(function(req)
    return { status = 200, json = { saw = req.params.id } }
  end)
  m:on{ route = "/t/:tenant/u/:user" }:reply(function(req)
    return { status = 200, json = { t = req.params.tenant, u = req.params.user } }
  end)

  t:expect(http.get(m.url .. "/orders/o-42"):json().saw):equals("o-42")
  local both = http.get(m.url .. "/t/acme/u/bob"):json()
  t:expect(both.t):equals("acme")
  t:expect(both.u):equals("bob")
end)

-- Segment-wise matching, which is the reason to have this rather than a hand-rolled `(.+)$`: that
-- pattern happily swallows a `/` and matches a sub-resource it was never meant to.
prova.test("a route param does not swallow a slash", function(t)
  local m = http.mock(t)
  m:on{ route = "/orders/:id" }:reply{ status = 200, body = "one" }

  t:expect(http.get(m.url .. "/orders/o-1").body):equals("one")
  t:expect(http.get(m.url .. "/orders/o-1/items").status):equals(404) -- a different route entirely
  t:expect(http.get(m.url .. "/orders/").status):equals(404)          -- an empty param is not a param
end)

-- A literal colon is legal in a path and real APIs use it (Google spells custom methods
-- `/v1/models/x:predict`). `path` staying exact is what keeps those expressible.
prova.test("path stays exact and does not interpret a colon", function(t)
  local m = http.mock(t)
  m:on{ path = "/v1/models/x:predict" }:reply{ status = 200, body = "predicted" }
  t:expect(http.get(m.url .. "/v1/models/x:predict").body):equals("predicted")
end)

prova.test("params appear in the journal too", function(t)
  local m = http.mock(t)
  m:on{ route = "/orders/:id" }:reply{ status = 200 }
  http.get(m.url .. "/orders/o-7")
  t:expect(m:received()[1].params.id):equals("o-7")
end)

prova.test("is grammar-shaped: url, host, port", function(t)
  local m = http.mock(t)
  t:expect(m.host):equals("127.0.0.1")
  t:expect(m.port):gt(1024)
  t:expect(m.url):equals("http://127.0.0.1:" .. m.port)
end)

prova.test("two mocks in one test get distinct ports", function(t)
  local a, b = http.mock(t), http.mock(t)
  t:expect(a.port ~= b.port):is_true()

  a:on{ path = "/who" }:reply{ status = 200, body = "a" }
  b:on{ path = "/who" }:reply{ status = 200, body = "b" }

  t:expect(http.get(a.url .. "/who").body):equals("a")
  t:expect(http.get(b.url .. "/who").body):equals("b")
  t:expect(a:received()):has_length(1) -- journals are per-mock, not shared
end)
