-- Files in a suite run in order (sorted); this one sets up and writes.
prova.test("creates the schema and inserts a row", function(t)
  local c = t:use("db")
  c:execute("CREATE TABLE IF NOT EXISTS orders (id BIGINT PRIMARY KEY, sku TEXT, qty INT)")
  c:execute("INSERT INTO orders (id, sku, qty) VALUES ($1, $2, $3)", { 1, "widget", 3 })
  t:expect(c:query_value("SELECT count(*) FROM orders")):equals(1)
end)
