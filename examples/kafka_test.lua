--- The `kafka` client + `kafka.container` recipe against a REAL single-node Kafka (KRaft) in an
--- ephemeral container. Run from the repo root: `prova examples/kafka_test.lua`. Requires docker;
--- skips otherwise. The recipe uses a fixed host port (Kafka advertises a reachable listener), so
--- only one kafka.container runs per host at a time.

local mq = prova.fixture("kafka", Scope.File, function(ctx)
  return kafka.container(ctx).client
end)

prova.group("kafka", { requires = { "docker" } }, function(g)
  g:test("produce and consume round-trips messages", function(t)
    local client = t:use(mq)
    local topic = "prova-demo"
    client:produce(topic, "hello")
    client:produce(topic, "world")
    -- A fresh consumer group with auto.offset.reset=earliest reads from the start of the topic.
    local msgs = client:consume(topic, { max = 2, timeout = "20s" })
    t:expect(#msgs):equals(2)
    t:expect(msgs):contains("hello")
    t:expect(msgs):contains("world")
  end)
end)
