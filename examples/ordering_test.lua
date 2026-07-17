--- Example: the ordering primitives — flow (ordered + shared context + skip-on-fail),
--- depends_on (DAG edge, skip-downstream-on-failure), and resource-gated concurrency.
---
--- Runnable as of `http.mock`: the service these primitives order calls against is a **stateful
--- fake**, so the example demonstrates the execution model against something that really answers.
--- It needs no docker, no network, and no build.
---
--- Note what the fake is not: it is not a mock of the system under test. Here the *primitives* are
--- what's being shown, and the service is scaffolding. When the system under test is the service,
--- run the real one (`prova.containerized`) — that is what the whole topology arc exists for.

--------------------------------------------------------------------------------------------
-- A stateful fake of the orders service.
--
-- This is `http.mock`'s raw capability and nothing more. A reply handler is *real Lua*, so the
-- "state" is an ordinary table the fixture closes over — no state API, no mini-language. And
-- because it is an ordinary table, the test can assert on it directly with the ordinary matchers.
--------------------------------------------------------------------------------------------
local api = prova.fixture("api", Scope.Suite, function(ctx)
  local orders, users, seq = {}, {}, 0
  local m = http.mock(ctx)

  m:on{ method = "POST", path = "/orders" }:reply(function(req)
    seq = seq + 1
    local id = "o-" .. seq
    orders[id] = { id = id, sku = req.json.sku, qty = req.json.qty, status = "open" }
    return { status = 201, json = orders[id] }
  end)

  -- `route` captures the id; the alternative is spelling this path twice — once as
  -- `path_matches = "^/orders/"` and once as `req.path:match("/orders/(.+)$")` — in two different
  -- languages that are free to drift apart.
  m:on{ method = "GET", route = "/orders/:id" }:reply(function(req)
    local o = orders[req.params.id]
    if not o then return { status = 404, json = { error = "no such order" } } end
    return { status = 200, json = o }
  end)

  m:on{ method = "POST", route = "/orders/:id/cancel" }:reply(function(req)
    local o = orders[req.params.id]
    if not o then return { status = 404 } end
    o.status = "cancelled"
    return { status = 204 }
  end)

  m:on{ method = "POST", path = "/admin/seed" }:reply(function()
    users["u-1"] = { id = "u-1", seeded = true }
    return { status = 200, json = { seeded = 1 } }
  end)

  m:on{ method = "GET", path = "/reports/summary" }:reply(function()
    local n = 0
    for _ in pairs(users) do n = n + 1 end
    return { status = 200, json = { records = n } }
  end)

  -- Hand the state back alongside the url: asserting on how the fake's world changed needs no API,
  -- because `orders` is just a Lua table.
  return { base = m.url, orders = orders, mock = m }
end)

--------------------------------------------------------------------------------------------
-- Flow: ordered steps sharing state. `read`/`cancel` are skipped if `create` fails.
--------------------------------------------------------------------------------------------
prova.flow("order lifecycle", { tags = { "acceptance" } }, function(flow)
  local order -- shared across steps via closure (this is the flow idiom)

  flow:step("create", function(t)
    local svc = t:use(api)
    order = http.post(svc.base .. "/orders", { json = { sku = "widget", qty = 2 } }):json()
    t:expect(order.id):is_truthy()
  end)

  flow:step("read back", function(t)
    local svc = t:use(api)
    local res = http.get(svc.base .. "/orders/" .. order.id)
    t:expect(res.status):equals(200)
    t:expect(res:json().qty):equals(2)
  end)

  flow:step("cancel", function(t)
    local svc = t:use(api)
    t:expect(http.post(svc.base .. "/orders/" .. order.id .. "/cancel").status):equals(204)
  end)

  -- Assert over the state change, not just the responses: the fake's world really moved.
  flow:step("the fake's state moved with it", function(t)
    local svc = t:use(api)
    t:expect(svc.orders[order.id].status):equals("cancelled")
    -- ...and the interaction is assertable too, which no real dependency would tell you.
    t:expect(svc.mock:received{ method = "POST", path = "/orders/" .. order.id .. "/cancel" })
      :has_length(1)
  end)
end)

--------------------------------------------------------------------------------------------
-- depends_on: a DAG edge. `report` is SKIPPED (not failed) if `seed` didn't pass.
-- Note: depends_on gates on pass/fail only — shared data flows through the fixture, not the edge.
--------------------------------------------------------------------------------------------
local seed = prova.test("seed reference data", { tags = { "slow" }, resources = { prova.shared("db") } },
  function(t)
    local svc = t:use(api)
    t:expect(http.post(svc.base .. "/admin/seed").status):equals(200)
  end)

prova.test("report reflects seed", { depends_on = { seed }, resources = { prova.shared("db") } },
  function(t)
    local svc = t:use(api)
    local res = http.get(svc.base .. "/reports/summary")
    t:expect(res.status):equals(200)
    t:expect(res:json().records):gt(0)
  end)

--------------------------------------------------------------------------------------------
-- A 404 path, which a stub table alone could not express: it depends on what was created.
--------------------------------------------------------------------------------------------
prova.test("an unknown order is a 404", function(t)
  local svc = t:use(api)
  local res = http.get(svc.base .. "/orders/o-does-not-exist")
  t:expect(res.status):equals(404)
  t:expect(res:json().error):equals("no such order")
end)
