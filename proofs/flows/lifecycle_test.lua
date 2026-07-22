-- Dogfoods flows: ordered steps sharing context, and a Scope.Flow fixture built once and shared
-- across the flow's steps. (Cascade-skip when a step does not pass is proven, green, in
-- proofs/ordering/depends_on.)

-- A Scope.Flow fixture: built on first use inside the flow, shared by every step.
local ledger = prova.fixture("ledger", Scope.Flow, function(ctx)
  return { entries = {} }
end)

prova.flow("order lifecycle", function(f)
  local order   -- shared across steps (the flow context — a closure upvalue)

  f:step("create", function(t)
    order = { id = "o-1", status = "open" }
    table.insert(t:use(ledger).entries, "created " .. order.id)
    t:expect(order.id):is_truthy()
  end)

  f:step("read back", function(t)
    t:expect(order.status):equals("open")                     -- sees what "create" set
    t:expect(t:use(ledger).entries):contains("created o-1")   -- the SAME ledger instance
  end)

  f:step("cancel", function(t)
    order.status = "cancelled"
    t:expect(order.status):equals("cancelled")
  end)
end)
