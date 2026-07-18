-- Dogfoods flows: ordered steps sharing context; a later step sees an earlier step's state.
prova.flow("order lifecycle", function(f)
  local order   -- shared across steps (the flow context)

  f:step("create", function(t)
    order = { id = "o-1", status = "open" }
    t:expect(order.id):is_truthy()
  end)

  f:step("read back", function(t)
    t:expect(order.status):equals("open")   -- sees what "create" set
  end)

  f:step("cancel", function(t)
    order.status = "cancelled"
    t:expect(order.status):equals("cancelled")
  end)
end)
