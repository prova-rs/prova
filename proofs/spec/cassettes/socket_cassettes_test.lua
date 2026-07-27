--- socket cassettes — record/replay on the L4 wiretap (docs/design/mocks-proxies-drivers.md).
--- FRAMING is what makes this possible: a framed `socket.proxy` already sees turns, so the VCR
--- discipline (request-turn selects recorded response-turn) holds exactly as it does for http.
--- Decisions these specs pin:
---
---   * `socket.proxy(ctx, { upstream?, framing, cassette?, mode? })` — the same modes as every
---     other transport: passthrough (default) | record | replay | auto. Replay needs no upstream.
---   * The match key is the framed request turn (exact bytes). A replay miss severs the
---     connection LOUD — inventing bytes for an unrecorded turn is the exact failure a
---     cassette exists to catch.
---   * Cassettes require framing, as mocks do: raw byte streams have no turn boundary. The
---     full-duplex/raw story is the scripted-conversation model, deliberately out of scope here.

local function upstream(t)
  local srv = socket.mock(t, { framing = "line" })
  srv:on("PING"):reply("PONG")
  return srv
end

prova.test("record mode captures framed turns; close is the flush point",
  { spec = "tier-a/socket-cassettes: record mode — not built" }, function(t)
  local srv = upstream(t)
  local cas = t:tempdir() .. "/turns.cassette"

  local p = socket.proxy(t, { upstream = srv.addr, framing = "line", cassette = cas, mode = "record" })
  local c = socket.connect(p.addr, { framing = "line" })
  c:send("PING")
  t:expect(c:recv()):equals("PONG")                -- flows while recording
  p:close()

  t:expect(fs.exists(cas)):is_true()
end)

prova.test("a proxy in record mode manufactures a mock — replay with the upstream GONE",
  { spec = "tier-a/socket-cassettes: replay mode — not built" }, function(t)
  local srv = upstream(t)
  local cas = t:tempdir() .. "/replay.cassette"

  local rec = socket.proxy(t, { upstream = srv.addr, framing = "line", cassette = cas, mode = "record" })
  local c = socket.connect(rec.addr, { framing = "line" })
  c:send("PING")
  t:expect(c:recv()):equals("PONG")
  rec:close()
  srv:stop()                                       -- reality leaves the building

  local rep = socket.proxy(t, { framing = "line", cassette = cas, mode = "replay" })
  local c2 = socket.connect(rep.addr, { framing = "line" })
  c2:send("PING")
  t:expect(c2:recv()):equals("PONG")               -- pinned deterministically forever
end)

prova.test("auto mode: record when the cassette is absent, replay when it is present",
  { spec = "tier-a/socket-cassettes: auto mode — not built" }, function(t)
  local srv = upstream(t)
  local cas = t:tempdir() .. "/auto.cassette"

  local first = socket.proxy(t, { upstream = srv.addr, framing = "line", cassette = cas, mode = "auto" })
  local c = socket.connect(first.addr, { framing = "line" })
  c:send("PING")
  t:expect(c:recv()):equals("PONG")
  first:close()
  t:expect(fs.exists(cas)):is_true()               -- absent → recorded

  srv:stop()
  local second = socket.proxy(t, { upstream = srv.addr, framing = "line", cassette = cas, mode = "auto" })
  local c2 = socket.connect(second.addr, { framing = "line" })
  c2:send("PING")
  t:expect(c2:recv()):equals("PONG")               -- present → replayed
end)

prova.test("a replay miss severs the connection loud — never invented bytes",
  { spec = "tier-a/socket-cassettes: loud replay miss — not built" }, function(t)
  local srv = upstream(t)
  local cas = t:tempdir() .. "/miss.cassette"

  local rec = socket.proxy(t, { upstream = srv.addr, framing = "line", cassette = cas, mode = "record" })
  local c = socket.connect(rec.addr, { framing = "line" })
  c:send("PING")
  t:expect(c:recv()):equals("PONG")
  rec:close()

  local rep = socket.proxy(t, { framing = "line", cassette = cas, mode = "replay" })
  local c2 = socket.connect(rep.addr, { framing = "line" })
  c2:send("NOPE")
  local ok = pcall(function() c2:recv{ timeout = "2s" } end)
  t:expect(ok):is_false()                          -- closed loud, not answered by guesswork
end)
