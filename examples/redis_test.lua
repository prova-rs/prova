--- The `redis` client + the `redis.container` recipe against a REAL Redis in an ephemeral container.
--- Run from the repo root: `prova examples/redis_test.lua`. Requires docker; skips otherwise.

local cache = prova.fixture("redis", "file", function(ctx)
  return redis.container(ctx).conn
end)

prova.group("redis", { requires = { "docker" } }, function(g)
  g:test("set / get round-trips a value", function(t)
    local r = t:use(cache)
    r:set("greeting", "hello")
    t:expect(r:get("greeting")):equals("hello")
    t:expect(r:get("missing")):is_nil()
  end)

  g:test("exists, incr, and del", function(t)
    local r = t:use(cache)
    t:expect(r:exists("counter")):is_false()
    t:expect(r:incr("counter")):equals(1)
    t:expect(r:incr("counter", 4)):equals(5)
    t:expect(r:exists("counter")):is_true()
    t:expect(r:del("counter")):equals(1)
    t:expect(r:exists("counter")):is_false()
  end)

  g:test("ping and the generic command escape hatch", function(t)
    local r = t:use(cache)
    t:expect(r:ping()):equals("PONG")
    r:command("SET", "color", "blue")
    t:expect(r:command("GET", "color")):equals("blue")
  end)
end)
