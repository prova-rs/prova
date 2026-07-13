--- The `pulsar` client + `pulsar.container` recipe against a REAL Pulsar standalone in an ephemeral
--- container. Run from the repo root: `prova examples/pulsar_test.lua`. Requires docker; skips
--- otherwise. Pulsar standalone is a heavy image and slow to start (tens of seconds on a cold pull).

local mq = prova.fixture("pulsar", "file", function(ctx)
  return pulsar.container(ctx).client
end)

prova.group("pulsar", { requires = { "docker" } }, function(g)
  g:test("produce and consume round-trips messages", function(t)
    local client = t:use(mq)
    local topic = "prova-demo"
    client:produce(topic, "hello")
    client:produce(topic, "world")
    -- Consumers read from the earliest offset, so messages produced before the subscription arrive.
    local msgs = client:consume(topic, { max = 2, timeout = "15s" })
    t:expect(#msgs):equals(2)
    t:expect(msgs[1]):equals("hello")
    t:expect(msgs[2]):equals("world")
  end)
end)
