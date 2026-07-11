-- The factory AWAITS (prova.sleep) — only possible now that ctx:use is an async method. Before this
-- increment the factory was called synchronously and could not drive an await.
local slow = prova.fixture("slow", "file", function(ctx)
  prova.sleep(20)              -- await inside a fixture factory
  ctx:defer(function() end)
  return 42
end)

-- A second fixture depends on the async one — the await chains through ctx:use.
local derived = prova.fixture("derived", "file", function(ctx)
  local base = ctx:use(slow)   -- awaits `slow`'s async factory
  return base + 1
end)

prova.test("async-built fixture resolves", function(t)
  t:expect(t:use(slow)):equals(42)
end)

prova.test("fixture that awaits a fixture", function(t)
  t:expect(t:use(derived)):equals(43)
end)
