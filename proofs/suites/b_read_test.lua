-- A DIFFERENT file, but the SAME store (built once for the suite), so it sees the row file `a`
-- inserted. That cross-file shared state is exactly what a suite is for.
prova.test("reads the row inserted by the other file in the suite", function(t)
  local s = t:use("store")
  t:expect(s.orders[1]):is_truthy() -- present, written by a_create_test
  t:expect(s.orders[1].sku):equals("widget")
  t:expect(s.orders[1].qty):equals(3)
end)
