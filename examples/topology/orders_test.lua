--- A TOPOLOGY — one definition, multiple consumers. The `orders` environment below (a seeded
--- Postgres + a Redis, wired together) powers BOTH the tests here AND `prova up orders`, so your
--- tests and your dev environment are the same description and cannot drift.
---
---   prova                 run the assertions against the topology (provision → assert → tear down)
---   prova up orders       stand it up, print endpoints, hold until Ctrl-C — psql / redis-cli into it
---   prova start orders    stand it up detached; `prova ps` to list, `prova down orders` to stop
---
--- `prova.topology` is a fixture (default Scope.File — provisioned once, shared across this file's
--- tests) that is ALSO addressable by name for the up/start verbs. requires docker (skips without it).

local postgres = require("postgres")
local redis = require("redis")

-- The definition. `ctx:manage` (inside the plugins' `.container`) ties teardown to the scope, so the
-- same code reaps everything whether a test ends or `prova down` signals a held environment.
local orders = prova.topology("orders", function(ctx)
  -- Postgres, seeded with a schema and a row, so `prova up orders` hands you a ready-to-use database.
  local db = postgres.container(ctx, { database = "orders" })
  db.client:execute("CREATE TABLE orders (id int primary key, sku text, qty int)")
  db.client:execute("INSERT INTO orders (id, sku, qty) VALUES (1, 'widget', 3)")

  -- Redis, wired to the same environment and seeded too.
  local cache = redis.container(ctx)
  cache.client:set("orders:1:sku", "widget")

  return { db = db, cache = cache } -- each is { client, url, container }; `up` prints their `url`s
end)

prova.group("orders topology", { requires = { "docker" } }, function(g)
  g:test("the database comes up seeded", function(t)
    local e = t:use(orders)
    t:expect(e.db.client:query_value("select count(*) from orders")):equals(1)
    t:expect(e.db.client:query_value("select sku from orders where id = $1", { 1 })):equals("widget")
  end)

  g:test("the cache is wired to the same environment", function(t)
    local e = t:use(orders) -- File-scoped: the same instance the sibling test used
    t:expect(e.cache.client:get("orders:1:sku")):equals("widget")
  end)
end)
