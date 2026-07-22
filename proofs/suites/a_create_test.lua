-- Files in a suite run in sorted order; this one writes a row into the suite's shared store.
prova.test("inserts a row into the shared store", function(t)
  local s = t:use("store")
  s.orders[1] = { sku = "widget", qty = 3 }
  t:expect(s.orders[1].sku):equals("widget")
end)
