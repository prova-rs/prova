--- The `socket` kernel transport — low-level byte streams with the full Mock/Proxy/Driver
--- triad (docs/design/mocks-proxies-drivers.md). Design decisions these specs pin:
---
---   * ONE namespace, unified by ADDRESS SCHEME, not by transport family: `tcp://host:port`
---     and `unix:///path` (future `npipe://` on Windows) are just addresses. Listen, connect,
---     proxy, and the byte model are identical across schemes; only address parsing differs.
---     A `unix://` address implicitly folds `requires = { "unix" }` into the leaf — authors
---     never hand-write the platform gate for a transport that knows its own platform.
---   * A raw byte stream has no natural "request" unit, so mocks and transcripts need a
---     FRAMING strategy to turn bytes into matchable turns: `"line"`, `{ length_prefixed = n }`
---     (n-byte big-endian length header), `{ delimiter = "..." }`, or a Lua chunker function.
---     No framing = raw bytes, driven by explicit recv sizes.
---   * The three postures: `socket.mock` (terminate — listen, answer synthetically),
---     `socket.proxy` (interpose — the wiretap; this is what gives EVERY TCP-based protocol
---     fault injection and transcripts with zero protocol knowledge), `socket.connect` /
---     `socket.listen` (originate — you are the traffic).
---   * Endpoint symmetry: a mock's `.addr` is the same string a driver's connect takes.
---   * Journals follow the §6 spine (seq/source/matched) from day one.

-- ── originate: the low-level driver (listen + connect, raw bytes) ────────────────────────────

prova.test("raw byte round-trip — connect, send bytes, accept, recv exact bytes",
  { proves = "tier-a/socket: the originate posture — raw bytes, exact counts, no ceremony" }, function(t)
  local srv = socket.listen(t, { addr = "tcp://127.0.0.1:0" })   -- :0 = ephemeral, addr resolves
  local c = socket.connect(srv.addr)

  c:send("\1\2\3")
  local conn = srv:accept()
  t:expect(conn:recv(3)):equals("\1\2\3")

  conn:send("\4")
  t:expect(c:recv(1)):equals("\4")
end)

-- ── terminate: socket.mock with framing ──────────────────────────────────────────────────────

prova.test("socket.mock answers framed turns — line framing over tcp",
  { proves = "tier-a/socket: terminate posture — framing turns bytes into matchable turns" }, function(t)
  local srv = socket.mock(t, { addr = "tcp://127.0.0.1:0", framing = "line" })
  srv:on("PING"):reply("PONG")

  local c = socket.connect(srv.addr, { framing = "line" })
  c:send("PING")
  t:expect(c:recv()):equals("PONG")
end)

prova.test("the same API over unix:// — schemes unify the transport family",
  { requires = { "unix" }, proves = "tier-a/socket: one namespace unified by address scheme, not transport family" }, function(t)
  local addr = "unix://" .. t:tempdir() .. "/app.sock"
  local srv = socket.mock(t, { addr = addr, framing = "line" })
  srv:on("PING"):reply("PONG")
  t:expect(srv.addr:sub(1, 7)):equals("unix://")   -- endpoint symmetry, scheme preserved

  local c = socket.connect(srv.addr, { framing = "line" })
  c:send("PING")
  t:expect(c:recv()):equals("PONG")
end)

prova.test("length-prefixed framing — a 4-byte big-endian header delimits turns",
  { proves = "tier-a/socket: length-prefixed framing — the header is the framing layer's business" }, function(t)
  local srv = socket.mock(t, { framing = { length_prefixed = 4 } })
  srv:on("hello"):reply("world")

  local c = socket.connect(srv.addr, { framing = { length_prefixed = 4 } })
  c:send("hello")                       -- the framing layer writes the header
  t:expect(c:recv()):equals("world")    -- and strips it on the way back
end)

prova.test("delimiter framing — any byte sequence can bound a turn",
  { proves = "tier-a/socket: delimiter framing — any byte sequence can bound a turn" }, function(t)
  local srv = socket.mock(t, { framing = { delimiter = "\0" } })
  srv:on("who"):reply("prova")

  local c = socket.connect(srv.addr, { framing = { delimiter = "\0" } })
  c:send("who")
  t:expect(c:recv()):equals("prova")
end)

prova.test("an unmatched turn is journaled loud — the §6 spine from day one",
  { proves = "tier-a/socket: the §6 spine from day one — unmatched turns are journaled loud" }, function(t)
  local srv = socket.mock(t, { framing = "line" })
  srv:on("KNOWN"):reply("OK")

  local c = socket.connect(srv.addr, { framing = "line" })
  c:send("NOPE")

  t:expect(function() return srv:received{ matched = false } end)
    :eventually{ timeout = "5s" }:has_length(1)
  t:expect(srv:received{ matched = false }[1].source):equals("unmatched")
end)

-- ── interpose: socket.proxy, the wiretap ─────────────────────────────────────────────────────

prova.test("socket.proxy interposes and records a direction-tagged transcript",
  { proves = "tier-a/socket: the interpose posture — the wiretap records direction-tagged turns" }, function(t)
  local srv = socket.mock(t, { framing = "line" })
  srv:on("PING"):reply("PONG")

  local tap = socket.proxy(t, { upstream = srv.addr, framing = "line" })
  local c = socket.connect(tap.addr, { framing = "line" })

  c:send("PING")
  t:expect(c:recv()):equals("PONG")     -- traffic flows through untouched

  local log = tap:transcript()          -- ordered turns, direction-tagged
  t:expect(log[1].dir):equals("up")     -- client → upstream
  t:expect(log[1].data):equals("PING")
  t:expect(log[2].dir):equals("down")   -- upstream → client
  t:expect(log[2].data):equals("PONG")
end)
