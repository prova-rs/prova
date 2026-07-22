--- POC example: the unit dependency DAG (`depends_on`).
---
--- Proves: units run only after their upstreams pass; two units sharing the same upstreams but
--- with no edge between them are independent; a failed/skipped upstream cascade-skips its whole
--- downstream (transitively), skip-not-fail (the TestNG behavior); `depends_on` accepts flows,
--- tests, and groups; and it gates on pass/fail only — data still flows through fixtures, not deps.

-- The classic shape: a login flow, then a populate flow that needs it, then two independent
-- journeys that both need login + populate but not each other.
local login = prova.flow("login", function(f)
  f:step("authenticate", function(t)
    t:expect(true):is_true()
  end)
end)

local populate = prova.flow("populate account", { depends_on = { login } }, function(f)
  f:step("seed profile", function(t) t:expect(1):equals(1) end)
  f:step("seed billing", function(t) t:expect(2):equals(2) end)
end)

-- Same upstreams, no edge between them → these two are independent of each other.
prova.flow("checkout journey", { depends_on = { login, populate } }, function(f)
  f:step("place order", function(t) t:expect("ok"):equals("ok") end)
end)

prova.test("settings journey", { depends_on = { login, populate } }, function(t)
  t:expect("saved"):equals("saved")
end)

-- Cascade: a failing upstream skips everything that (transitively) depends on it.
local boot = prova.test("boot service", function(t)
  t:expect("up"):equals("down")   -- deliberate failure
end)

-- Directly depends on the failed unit → skipped, not failed.
prova.test("probe health", { depends_on = { boot } }, function(t)
  error("must never run — boot failed")
end)

-- Depends on a group whose only child depends on the failed unit → also skipped (transitive).
local downstream = prova.group("downstream", { depends_on = { boot } }, function(g)
  g:test("consume", function(t) error("must never run — boot failed") end)
end)

prova.test("report", { depends_on = { downstream } }, function(t)
  error("must never run — downstream skipped")
end)
