--- The `mysql.container` recipe: the SAME query API as Postgres/SQLite against a real MySQL in an
--- ephemeral container, provisioned in one line. Run from the repo root: `prova examples/mysql_test.lua`.
--- Requires docker; skips gracefully otherwise. Note the only backend differences are the recipe call
--- and MySQL's `?` placeholders.

local my = prova.fixture("mysql", Scope.File, function(ctx)
  return mysql.container(ctx, { database = "orders" }).client
end)

prova.group("mysql", { requires = { "docker" } }, function(g)
  g:test("round-trips typed rows and aggregates", function(t)
    local c = t:use(my)
    c:execute("CREATE TABLE IF NOT EXISTS orders (id BIGINT PRIMARY KEY, sku TEXT, qty INT, price DOUBLE)")
    t:expect(c:execute("INSERT INTO orders (id, sku, qty, price) VALUES (?, ?, ?, ?)",
             { 1, "widget", 3, 9.99 })):equals(1)
    c:execute("INSERT INTO orders (id, sku, qty, price) VALUES (?, ?, ?, ?)", { 2, "gadget", 1, 4.50 })

    local rows = c:query("SELECT id, sku, qty FROM orders ORDER BY id")
    t:expect(#rows):equals(2)
    t:expect(rows[1].sku):equals("widget")
    t:expect(rows[1].qty):equals(3)

    t:expect(c:query_value("SELECT count(*) FROM orders")):equals(2)
    t:expect(c:query_value("SELECT sku FROM orders WHERE id = ?", { 2 })):equals("gadget")
    t:expect(c:query_value("SELECT sku FROM orders WHERE id = ?", { 99 })):is_nil()
  end)
end)
