-- Dogfoods depends_on: the unit dependency DAG. Units run only after their upstreams pass; a
-- failed/skipped upstream cascade-skips its whole downstream (transitively), skip-not-fail;
-- depends_on accepts flows, tests, and groups, and gates on pass/fail only.

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

-- Cascade: an upstream that does not PASS skips everything that (transitively) depends on it. We use
-- a SKIPPED upstream (an unmet requirement) rather than a failing one — same propagation, and the
-- proof stays green (a deliberate failure would redden the suite; skip-not-fail is the whole point).
local boot = prova.test("boot service", { requires = { "capability-nothing-provides" } }, function(t)
  error("must never run — its requirement is unmet, so it SKIPS")
end)

-- Directly depends on the skipped unit → skipped too.
prova.test("probe health", { depends_on = { boot } }, function(t)
  error("must never run — boot skipped")
end)

-- Depends on a group whose only child depends on the skipped unit → also skipped (transitive).
local downstream = prova.group("downstream", { depends_on = { boot } }, function(g)
  g:test("consume", function(t) error("must never run — boot skipped") end)
end)

prova.test("report", { depends_on = { downstream } }, function(t)
  error("must never run — downstream skipped")
end)
