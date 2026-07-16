--- Example: units as scheduling atoms + flow-to-flow dependencies (a diamond).
---
--- A `login` flow, then `populate` (needs login), then two journeys that each need login +
--- populate but NOT each other — so the two journeys run in parallel. Illustrates that flows
--- are isolated among themselves unless an edge says otherwise, and that a failed upstream
--- SKIPS (not fails) everything downstream. No real endpoints; illustrative of the model.

local api = prova.fixture("api_base", Scope.Suite, function(ctx)
  return "http://localhost:8080"
end)

-- Shared account state flows through this fixture, NOT through depends_on edges.
local account = prova.fixture("account", Scope.Suite, function(ctx)
  return { id = nil, token = nil }             -- populated by the login/populate flows
end)

--------------------------------------------------------------------------------------------
-- login: the root of the graph. Everything else depends (transitively) on it.
--------------------------------------------------------------------------------------------
local login = prova.flow("login", { resources = { prova.shared("auth") } }, function(f)
  f:step("authenticate", function(t)
    local base, acct = t:use(api), t:use(account)   -- scope-cached: same instances across steps
    local res = http.post(base .. "/auth/login", { json = { user = "demo", pass = "demo" } })
    t:expect(res.status):equals(200)
    acct.token = res:json().token
    t:expect(acct.token):is_truthy()
  end)
end)

--------------------------------------------------------------------------------------------
-- populate: depends on login; seeds data the journeys will read.
--------------------------------------------------------------------------------------------
local populate = prova.flow("populate account", { depends_on = { login } }, function(f)
  f:step("create profile", function(t)
    local base, acct = t:use(api), t:use(account)
    acct.id = http.post(base .. "/users", { json = { token = acct.token } }):json().id
    t:expect(acct.id):is_truthy()
  end)
  f:step("seed billing", function(t)
    local base, acct = t:use(api), t:use(account)
    t:expect(http.post(base .. "/users/" .. acct.id .. "/billing").status):equals(201)
  end)
end)

--------------------------------------------------------------------------------------------
-- Two journeys: same upstreams, no edge between them → they run in parallel (under --jobs).
-- If login or populate fails, BOTH are skipped with the reason — no spurious cascade.
--------------------------------------------------------------------------------------------
prova.flow("checkout journey", { depends_on = { login, populate }, tags = { "acceptance" } }, function(f)
  f:step("add to cart", function(t)
    local base, acct = t:use(api), t:use(account)
    t:expect(http.post(base .. "/carts/" .. acct.id .. "/items", { json = { sku = "widget" } }).status):equals(200)
  end)
  f:step("checkout", function(t)
    local base, acct = t:use(api), t:use(account)
    t:expect(http.post(base .. "/carts/" .. acct.id .. "/checkout").status):equals(200)
  end)
end)

prova.flow("settings journey", { depends_on = { login, populate }, tags = { "acceptance" } }, function(f)
  f:step("update email", function(t)
    local base, acct = t:use(api), t:use(account)
    t:expect(http.put(base .. "/users/" .. acct.id, { json = { email = "demo@example.com" } }).status):equals(200)
  end)
end)
