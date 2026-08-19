--- `websocket.proxy` — the interpose posture for ws, the last empty cell in the transport
--- matrix (docs/design/mocks-proxies-drivers.md). Message turns are natively framed, so the
--- wiretap records turns with no framing strategy; the fault vocabulary rides the substrate.
--- Cassette record/replay for ws is deliberately NOT here: full-duplex replay is the
--- scripted-conversation model, staged separately once the conversation format is designed.

prova.test("interpose + transcript — the ws wiretap records direction-tagged message turns",
  { proves = "tier-a/websocket.proxy: the ws wiretap records direction-tagged message turns" }, function(t)
  local m = websocket.mock(t)
  m:on("ping"):reply("pong")

  local p = websocket.proxy(t, { upstream = m.url })
  t:expect(p.url:sub(1, 5)):equals("ws://")        -- endpoint symmetry

  local c = websocket.connect(t, { url = p.url })
  c:send("ping")
  t:expect(c:recv()):equals("pong")                -- traffic flows through untouched

  local log = p:transcript()
  t:expect(log[1].dir):equals("up")
  t:expect(log[1].data):equals("ping")
  t:expect(log[2].dir):equals("down")
  t:expect(log[2].data):equals("pong")
end)

prova.test("the fault vocabulary rides the substrate — latency on the ws proxy",
  { proves = "tier-a/websocket.proxy: one fault vocabulary across transports — latency on the ws proxy" }, function(t)
  local m = websocket.mock(t)
  m:on("ping"):reply("pong")

  local p = websocket.proxy(t, { upstream = m.url })
  local c = websocket.connect(t, { url = p.url })
  c:send("ping")
  t:expect(c:recv{ timeout = "2s" }):equals("pong")   -- baseline

  p:latency("2s")
  c:send("ping")
  local ok = pcall(function() c:recv{ timeout = "200ms" } end)
  t:expect(ok):is_false()
end)
