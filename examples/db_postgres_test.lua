--- The North Star data layer: the SAME `db` API driving a REAL Postgres in an ephemeral container.
--- Run from the repo root: `prova examples/db_postgres_test.lua`. Requires docker; skips gracefully
--- where it is unavailable. Note the only difference from the SQLite example is the connect URL and
--- `$1` placeholders — the query surface is identical.

local pg = prova.fixture("pg", "file", function(ctx)
  -- `ctx:manage` ties the container's lifecycle to the fixture scope (stopped on teardown) — no
  -- `ctx:defer(function() c:stop() end)` closure.
  local c = ctx:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_PASSWORD = "secret", POSTGRES_DB = "orders" },
    ports = { 5432 },
    wait = { port = 5432, timeout = "60s" },
  })

  -- Postgres restarts once during first-boot init, so port-open (even pg_isready) can false-positive.
  -- `prova.retry` gates on the real connection actually holding — no hand-rolled loop.
  local url = "postgres://postgres:secret@127.0.0.1:" .. c:host_port(5432) .. "/orders"
  return ctx:manage(prova.retry(function() return db.connect(url) end,
    { timeout = "30s", message = "postgres never accepted connections" }))
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
