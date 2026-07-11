--- POC example: flows — ordered steps, shared context, flow-scoped fixtures, cascade-skip.
--- The target the second Rust prototype should run end to end.
---
--- Proves: steps run in declared order sharing closure state; a `flow`-scoped fixture is built
--- once and shared across steps (then torn down after the flow); a failed step cascade-skips the
--- rest of its flow; and independent flows are isolated from one another.

-- A flow-scoped fixture: built on first use inside the flow, shared by every step, torn down
-- after the flow's last step. `test` scope inside a flow means per-step, so this is the level
-- at which built-up flow state lives as a *fixture* (vs. a raw closure upvalue).
local ledger = prova.fixture("ledger", "flow", function(ctx)
  ctx:log("ledger opened")
  ctx:defer(function() ctx:log("ledger closed") end)
  return { entries = {} }
end)

prova.flow("order lifecycle", function(f)
  local order            -- shared by all steps (the flow context — a plain closure upvalue)

  f:step("create", function(t)
    order = { id = 42, qty = 2 }
    local l = t:use(ledger)
    table.insert(l.entries, "created " .. order.id)
    t:expect(order.id):is_truthy()
  end)

  f:step("read back", function(t)                 -- runs only if "create" passed
    t:expect(order.qty):equals(2)                 -- reads the shared upvalue
    local l = t:use(ledger)                        -- SAME ledger instance as the first step
    t:expect(l.entries):contains("created 42")
  end)

  f:step("cancel", function(t)
    t:expect(order.id):equals(42)
  end)
end)

-- A flow whose second step fails: the third step must cascade-skip, not fail.
prova.flow("cascade on failure", function(f)
  f:step("first ok", function(t)
    t:expect(1):equals(1)
  end)

  f:step("second fails", function(t)
    t:expect(1):equals(2)                         -- deliberate failure
  end)

  f:step("third is skipped", function(t)
    error("this step must never run")             -- proves cascade-skip
  end)
end)
