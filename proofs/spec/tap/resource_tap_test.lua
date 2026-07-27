--- Resource tapping — the tap option on `prova.containerized` resources (docs/design/
--- mocks-proxies-drivers.md + ecosystem.md). The whole point of the byte-level socket.proxy:
--- interpose on a REAL dependency with zero protocol knowledge. Decisions these specs pin:
---
---   * `X.container(ctx, { tap = true })` — a per-use choice by the CALLER, not the recipe
---     author: any resource built on `prova.containerized` can be tapped, none must be.
---   * With the tap on, `res.url` already routes THROUGH the proxy — the SUT wires in the same
---     url it always would and needs no knowledge the tap exists — and `res.tap` is the
---     socket.proxy handle: `:transcript()`, `:latency()`, `:drop()`, the whole vocabulary.
---   * The recipe keeps its own contract: readiness still probes the REAL container port (a
---     tap must never make a dead resource look alive).
---
--- Redis is the probe body (tiny image, RESP is \r\n-delimited so the delimiter framing reads
--- clean turns) — but the contract under proof is containerized+tap, not redis.

local redis = prova.containerized{
  name = "redis-tap-spec", image = "redis", tag = "7-alpine", port = 6379,
  url = function(hp) return "tcp://127.0.0.1:" .. hp end,
  -- The recipe author declares the wire framing once; `tap = true` then yields turn-level
  -- transcripts for free (RESP is \r\n-delimited).
  framing = { delimiter = "\r\n" },
}

prova.test("tap = true interposes the wiretap — same url shape, transcripts for free",
  { proves = "tier-a/tap: tap=true interposes the wiretap — same url shape, transcripts for free" }, function(t)
  local res = redis.container(t, { tap = true })

  local c = socket.connect(res.url, { framing = { delimiter = "\r\n" } })
  c:send("PING")
  t:expect(c:recv()):equals("+PONG")               -- the real redis answered, through the tap

  local log = res.tap:transcript()                 -- the proxy handle, vocabulary and all
  t:expect(log[1].dir):equals("up")
  t:expect(log[1].data):equals("PING")
  t:expect(log[2].dir):equals("down")
  t:expect(log[2].data):equals("+PONG")
end)

prova.test("a tapped resource takes faults — resilience proofs against the real dependency",
  { proves = "tier-a/tap: faults through the tap — resilience proofs against the real dependency" }, function(t)
  local res = redis.container(t, { tap = true })

  local c = socket.connect(res.url, { framing = { delimiter = "\r\n" } })
  c:send("PING")
  t:expect(c:recv()):equals("+PONG")               -- baseline: healthy

  res.tap:latency("2s")                            -- injure the wire, not the container
  c:send("PING")
  local ok = pcall(function() c:recv{ timeout = "200ms" } end)
  t:expect(ok):is_false()
end)
