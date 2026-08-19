--- Fault injection — the shared vocabulary on the proxy substrate (docs/design/
--- mocks-proxies-drivers.md). The interpose posture is the only one that can prove RESILIENCE
--- rather than the happy path. One vocabulary — `latency`, `drop`, `corrupt`, `throttle`,
--- `after` — lives once on the proxy substrate; any stream transport's proxy applies it.
--- toxiproxy's verbs, in-process, no extra daemon.
---
--- Vocabulary line held by api-freeze.md §7: `delay` remains the mocks' per-reply ONE-SHOT
--- (shipped); the fault verbs are CONTINUOUS stream conditions on proxies. Both words, distinct
--- meanings — a proxy has no `delay`, a mock has no `latency`.
---
--- Faults are specified BEHAVIORALLY (observable timeouts / severed connections / altered
--- bytes), never by reading clocks — the same discipline as `wait_for`/`expect`.

local function echo_upstream(t)
  local srv = socket.mock(t, { framing = "line" })
  srv:on("PING"):reply("PONG")
  return srv
end

prova.test("latency delays the stream — a tight recv timeout now trips",
  { proves = "tier-a/faults: latency is continuous and observable, never a clock read" }, function(t)
  local srv = echo_upstream(t)
  local p = socket.proxy(t, { upstream = srv.addr, framing = "line" })
  local c = socket.connect(t, { addr = p.addr, framing = "line" })

  c:send("PING")
  t:expect(c:recv{ timeout = "2s" }):equals("PONG")     -- baseline: clean pass-through

  p:latency("2s")                                        -- continuous, both directions
  c:send("PING")
  local ok = pcall(function() c:recv{ timeout = "200ms" } end)
  t:expect(ok):is_false()                                -- the fault is observable, now
end)

prova.test("drop severs live connections — recv fails loud, not hangs",
  { proves = "tier-a/faults: drop severs loud — a fault is an error, not a hang" }, function(t)
  local srv = echo_upstream(t)
  local p = socket.proxy(t, { upstream = srv.addr, framing = "line" })
  local c = socket.connect(t, { addr = p.addr, framing = "line" })

  c:send("PING")
  t:expect(c:recv()):equals("PONG")

  p:drop()                                               -- kill existing + refuse new
  local ok = pcall(function()
    c:send("PING")
    c:recv{ timeout = "1s" }
  end)
  t:expect(ok):is_false()
end)

prova.test("after() puts a fuse on any fault — healthy first, injured later",
  { proves = "tier-a/faults: after() is the fuse — resilience proofs need healthy-then-injured" }, function(t)
  local srv = echo_upstream(t)
  local p = socket.proxy(t, { upstream = srv.addr, framing = "line" })
  local c = socket.connect(t, { addr = p.addr, framing = "line" })

  p:after("500ms"):drop()

  c:send("PING")
  t:expect(c:recv()):equals("PONG")                      -- before the fuse: healthy

  t:expect(function()
    return pcall(function()
      local c2 = socket.connect(t, { addr = p.addr, framing = "line" })
      c2:send("PING")
      c2:recv{ timeout = "200ms" }
    end)
  end):eventually{ timeout = "5s" }:is_false()           -- after the fuse: dead
end)

prova.test("corrupt alters bytes in flight — what arrives is not what was sent",
  { proves = "tier-a/faults: corrupt alters bytes in flight, length preserved" }, function(t)
  local srv = socket.listen(t, { addr = "tcp://127.0.0.1:0" })
  local p = socket.proxy(t, { upstream = srv.addr })     -- raw, no framing
  p:corrupt()

  local payload = string.rep("A", 1024)
  local c = socket.connect(t, { addr = p.addr })
  c:send(payload)

  local conn = srv:accept()
  local got = conn:recv(1024)
  t:expect(#got):equals(1024)                            -- same length…
  t:expect(got):never():equals(payload)                  -- …different bytes
end)

prova.test("throttle rate-limits the stream — bulk transfer misses a tight deadline",
  { proves = "tier-a/faults: throttle rate-limits the stream, observable via deadlines" }, function(t)
  local srv = socket.mock(t, { framing = { length_prefixed = 4 } })
  local bulk = string.rep("x", 64 * 1024)
  srv:on("GET"):reply(bulk)

  local p = socket.proxy(t, { upstream = srv.addr, framing = { length_prefixed = 4 } })
  local c = socket.connect(t, { addr = p.addr, framing = { length_prefixed = 4 } })

  c:send("GET")
  t:expect(c:recv{ timeout = "5s" }):equals(bulk)        -- baseline: 64k arrives easily

  p:throttle("8kbps")                                    -- 64k at 8kbps ≈ minutes
  c:send("GET")
  local ok = pcall(function() c:recv{ timeout = "500ms" } end)
  t:expect(ok):is_false()
end)

prova.test("the vocabulary is transport-generic — http.proxy speaks the same verbs",
  { proves = "tier-a/faults: one vocabulary across transports — http.proxy speaks the same verbs" }, function(t)
  local m = http.mock(t)
  m:on{ path = "/ok" }:reply{ status = 200 }

  local p = http.proxy(t, { upstream = m.url })
  t:expect(http.get(p.url .. "/ok").status):equals(200)  -- baseline through the proxy

  p:latency("2s")
  local ok = pcall(function() http.get(p.url .. "/ok", { timeout = "300ms" }) end)
  t:expect(ok):is_false()
end)
