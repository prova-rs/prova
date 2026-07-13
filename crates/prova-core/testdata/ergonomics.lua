-- prova.retry (readiness without the hand-rolled loop) and ctx:manage (lifecycle without the
-- `ctx:defer(function() x:stop() end)` closure).

prova.test("retry returns once the condition holds", function(t)
  local attempts = 0
  local v = prova.retry(function()
    attempts = attempts + 1
    if attempts < 3 then error("not yet") end   -- raising is treated as "not ready"
    return "ready"
  end, { every = "1ms", timeout = "5s" })
  t:expect(v):equals("ready")
  t:expect(attempts):equals(3)
end)

prova.test("retry accepts a truthy return (not just non-raising)", function(t)
  local n = 0
  local v = prova.retry(function()
    n = n + 1
    return n >= 2 and n or nil   -- nil → not ready, so it retries
  end, { every = "1ms", timeout = "5s" })
  t:expect(v):equals(2)
end)

prova.test("retry times out with its message", function(t)
  local ok, err = pcall(function()
    prova.retry(function() return false end, { every = "1ms", timeout = "20ms", message = "never ready" })
  end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):contains("never ready")
end)

-- ctx:manage — a resource is just a table with stop()/close() here; teardown runs after each test.
local log = { stopped = 0, closed = 0 }

prova.test("manage returns the resource and stops it at scope end", function(t)
  local fake = { stop = function() log.stopped = log.stopped + 1 end }
  local r = t:manage(fake)
  t:expect(r == fake):is_true()    -- returns the SAME resource (identity, not deep-equals)
  t:expect(log.stopped):equals(0)  -- not yet — teardown runs after the body
end)

prova.test("the managed resource was stopped after the previous test", function(t)
  t:expect(log.stopped):equals(1)
end)

prova.test("manage falls back to close()", function(t)
  t:manage({ close = function() log.closed = log.closed + 1 end })
  t:expect(log.closed):equals(0)
end)

prova.test("the managed resource was closed after the previous test", function(t)
  t:expect(log.closed):equals(1)
end)

prova.test("manage rejects a resource with no stop()/close()", function(t)
  local ok = pcall(function() t:manage({}) end)
  t:expect(ok):is_false()
end)
