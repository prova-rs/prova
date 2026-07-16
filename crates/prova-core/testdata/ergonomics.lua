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
  t:expect(r):is(fake)             -- returns the SAME resource (identity; deep-equals can't, fake has a fn field)
  t:expect(log.stopped):equals(0)  -- not yet — teardown runs after the body
end)

prova.test("is asserts identity, not structure", function(t)
  local a = { x = 1 }
  local b = { x = 1 }
  t:expect(a):is(a)                -- same reference
  t:expect(a):never():is(b)        -- structurally equal, but a different table
  t:expect(a):equals(b)            -- ...whereas deep-equals treats them as equal
  t:expect(42):is(42)              -- primitives compare by value
  t:expect("hi"):is("hi")
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

-- Quiet-primitives surface: env scalars coerce, check=true errors carry both streams, spawned
-- processes capture output (bounded) via proc:output().

prova.test("env accepts numbers and booleans", function(t)
  local r = shell.run("echo $PORT $FLAG", { env = { PORT = 8080, FLAG = true } })
  t:expect(r.stdout):contains("8080 true")
end)

prova.test("check=true failures carry stdout and stderr", function(t)
  local ok, err = pcall(function()
    shell.run("echo out-detail; echo err-detail 1>&2; exit 3", { check = true })
  end)
  t:expect(ok):is_false()
  local msg = tostring(err)
  t:expect(msg):contains("exited 3")
  t:expect(msg):contains("err-detail")
  t:expect(msg):contains("out-detail")
end)

prova.test("spawned process output is captured", function(t)
  local proc = t:manage(shell.spawn("echo hello-from-spawn && sleep 5"))
  prova.retry(function()
    if proc:output():find("hello-from-spawn", 1, true) then return true end
  end, { timeout = "10s", message = "spawn output never captured" })
  t:expect(proc:output()):contains("hello-from-spawn")
end)
