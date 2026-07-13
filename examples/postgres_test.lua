--- The North Star data layer: the SAME query API driving a REAL Postgres in an ephemeral container.
--- Run from the repo root: `prova examples/postgres_test.lua`. Requires docker; skips gracefully
--- where it is unavailable. Note the only difference from the SQLite example is the client URL and
--- `$1` placeholders — the query surface is identical.

local pg = prova.fixture("pg", Scope.File, function(ctx)
  -- The `postgres.container` recipe folds the whole dance — provision an ephemeral container, wait for
  -- it to actually accept connections, open a managed connection — into one line. Returns { client,
  -- url, container }; here we just want the client.
  return postgres.container(ctx, { database = "orders" }).client
end)

prova.group("postgres", { requires = { "docker" } }, function(g)
  g:test("round-trips typed rows and aggregates", function(t)
    local c = t:use(pg)
    c:execute("CREATE TABLE IF NOT EXISTS orders (id BIGINT PRIMARY KEY, sku TEXT, qty INT, price REAL)")
    t:expect(c:execute("INSERT INTO orders (id, sku, qty, price) VALUES ($1, $2, $3, $4)",
             { 1, "widget", 3, 9.99 })):equals(1)
    c:execute("INSERT INTO orders (id, sku, qty, price) VALUES ($1, $2, $3, $4)", { 2, "gadget", 1, 4.50 })

    local rows = c:query("SELECT id, sku, qty FROM orders ORDER BY id")
    t:expect(#rows):equals(2)
    t:expect(rows[1].sku):equals("widget")
    t:expect(rows[1].qty):equals(3)

    t:expect(c:query_value("SELECT count(*) FROM orders")):equals(2)
    t:expect(c:query_value("SELECT sku FROM orders WHERE id = $1", { 2 })):equals("gadget")
    t:expect(c:query_value("SELECT sku FROM orders WHERE id = $1", { 99 })):is_nil()
  end)
end)
