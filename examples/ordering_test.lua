--- Example: the ordering primitives — flow (ordered + shared context + skip-on-fail),
--- depends_on (DAG edge, skip-downstream-on-failure), and resource-gated concurrency.
--- No archetect/network; illustrative of the execution model, not runnable yet.

-- A fixture the flow will build once for its whole lifetime (flow scope).
local api = assay.fixture("api_base", "suite", function(ctx)
  return "http://localhost:8080"
end)

--------------------------------------------------------------------------------------------
-- Flow: ordered steps sharing state. `read`/`delete` are skipped if `create` fails.
--------------------------------------------------------------------------------------------
assay.flow("order lifecycle", { tags = { "acceptance" }, requires = { "network" }, resources = { assay.port(8080) } }, function(flow)
  local base = flow:use(api)     -- flow-scoped fixture value
  local order                    -- shared across steps via closure

  flow:step("create", function(t)
    order = http.post(base .. "/orders", { json = { sku = "widget", qty = 2 } }):json()
    t:expect(order.id):is_truthy()
  end)

  flow:step("read back", function(t)
    local res = http.get(base .. "/orders/" .. order.id)
    t:expect(res.status):equals(200)
    t:expect(res:json().qty):equals(2)
  end)

  flow:step("cancel", function(t)
    t:expect(http.post(base .. "/orders/" .. order.id .. "/cancel").status):equals(204)
  end)
end)

--------------------------------------------------------------------------------------------
-- depends_on: a DAG edge. `report` is SKIPPED (not failed) if `seed` didn't pass.
-- Note: depends_on gates on pass/fail only — shared data flows through the fixture, not the edge.
--------------------------------------------------------------------------------------------
local seed = assay.test("seed reference data", { tags = { "slow" }, resources = { assay.shared("db") } }, function(t)
  local base = t:use(api)
  t:expect(http.post(base .. "/admin/seed").status):equals(200)
end)

assay.test("report reflects seed", { depends_on = { seed }, resources = { assay.shared("db") } }, function(t)
  local base = t:use(api)
  local res = http.get(base .. "/reports/summary")
  t:expect(res.status):equals(200)
  t:expect(res:json().records):gt(0)
end)
