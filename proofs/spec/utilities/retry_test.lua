--- `prova.retry` — the readiness primitive, and what it says when readiness never arrives.
---
--- The contract is "call until it returns TRUTHY". The failure message is what an author actually
--- debugs against, and it was misleading in two ways that cost a caller real time (both fixed here):
--- a stale error reported as the current state, and silence about the commonest mistake of all —
--- a closure that asserts and forgets to return.

prova.test("retry returns the first truthy value", function(t)
  local n = 0
  local got = prova.retry(function()
    n = n + 1
    if n < 3 then
      return nil
    end
    return "ready:" .. n
  end, { timeout = "5s", every = "10ms" })
  t:expect(got):equals("ready:3")
end)

prova.test("a closure that never returns anything is TOLD so, not just 'not met'", function(t)
  -- The commonest mistake: a closure that only asserts. It raises while the condition is false, then
  -- stops raising and returns nil — so `retry` spins to the deadline on a condition that is, in fact,
  -- already met. "condition not met" alone reads as "your system never got there" and sends the
  -- author to debug the system. The message must name the real cause.
  local ok, err = pcall(function()
    prova.retry(function()
      -- asserts nothing, returns nothing: always "not ready"
    end, { timeout = "200ms", every = "20ms" })
  end)
  t:expect(ok, "retry should have failed"):equals(false)
  t:expect(tostring(err)):contains("never returned a truthy value")
  t:expect(tostring(err), "the fix, stated"):contains("return true")
end)

prova.test("an error that stopped happening is not reported as the current state", function(t)
  -- `last_err` used to be sticky. A closure that raised early and merely returned nil afterwards
  -- reported that first error at the deadline — describing a world that had ceased to exist several
  -- seconds earlier. That is worse than no detail: it is confidently wrong detail.
  local n = 0
  local ok, err = pcall(function()
    prova.retry(function()
      n = n + 1
      if n == 1 then
        error("the transient failure")
      end
      -- thereafter: no error, but still nothing truthy
    end, { timeout = "200ms", every = "20ms" })
  end)
  t:expect(ok, "retry should have failed"):equals(false)
  t:expect(tostring(err), "stale error"):never():contains("the transient failure")
end)

prova.test("a raise on the LAST attempt is still reported", function(t)
  -- The other half: when the closure is failing right now, its error is the most useful thing there
  -- is, and clearing stale errors must not lose it.
  local ok, err = pcall(function()
    prova.retry(function()
      error("still broken")
    end, { timeout = "200ms", every = "20ms" })
  end)
  t:expect(ok, "retry should have failed"):equals(false)
  t:expect(tostring(err)):contains("still broken")
end)
