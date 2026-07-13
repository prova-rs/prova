-- A DIFFERENT file — but the SAME Postgres (one container for the suite), so it sees the row the
-- other file inserted. That cross-file shared state is exactly what a suite is for.
prova.test("reads the row inserted by the other file in the suite", function(t)
  local c = t:use("db")
  t:expect(c:query_value("SELECT sku FROM orders WHERE id = $1", { 1 })):equals("widget")
  t:expect(c:query_value("SELECT qty FROM orders WHERE id = $1", { 1 })):equals(3)
end)
