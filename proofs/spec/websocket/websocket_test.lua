--- The `websocket` transport — full-duplex messaging over the http upgrade path. Design
--- decisions pinned:
---
---   * The protocol frames messages natively, so unlike raw `socket` there is NO framing
---     strategy: a turn is a ws message, always.
---   * Full-duplex means the mock is not only request→response: `on_connect` lets the server
---     side PUSH unprompted — the scripted-conversation model, not VCR.
---   * Journals speak the §6 spine (seq/source/matched) from day one — no retrofit later.
---   * `websocket` joins the reserved-name registry alongside `socket` and `terminal`.

prova.test("mock + driver round-trip — a message turn, stubbed and answered",
  { proves = "tier-a/websocket: message turns, natively framed — no framing strategy to choose" }, function(t)
  local m = websocket.mock(t)
  m:on("ping"):reply("pong")
  t:expect(m.url:sub(1, 5)):equals("ws://")     -- endpoint symmetry, ws scheme

  local c = websocket.connect(t, { url = m.url })
  c:send("ping")
  t:expect(c:recv()):equals("pong")
end)

prova.test("full duplex — the server pushes unprompted on connect",
  { proves = "tier-a/websocket: full duplex — the scripted-conversation model, not VCR" }, function(t)
  local m = websocket.mock(t)
  m:on_connect(function(conn) conn:send("welcome") end)

  local c = websocket.connect(t, { url = m.url })
  t:expect(c:recv()):equals("welcome")          -- no request preceded this
end)

prova.test("the journal speaks the §6 spine from day one",
  { proves = "tier-a/websocket: the §6 spine from day one — no retrofit" }, function(t)
  local m = websocket.mock(t)
  m:on("hello"):reply("hi")

  local c = websocket.connect(t, { url = m.url })
  c:send("hello")
  t:expect(c:recv()):equals("hi")
  c:send("unexpected")

  t:expect(function() return m:received() end):eventually{ timeout = "5s" }:has_length(2)
  local e = m:received()
  t:expect(e[1].seq):equals(1)
  t:expect(e[1].matched):is_true()
  t:expect(e[1].source):equals("stub")
  t:expect(e[2].matched):is_false()
  t:expect(e[2].source):equals("unmatched")     -- unmatched is journaled, not dropped
end)
