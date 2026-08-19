--- The cohesion pass — the seams that keep the transport surface from reading as one API
--- (docs/design/mocks-proxies-drivers.md). Three small, load-bearing consistencies:
---
---   1. grpc.proxy speaks the fault vocabulary. Every other interpose posture can inject latency;
---      grpc.proxy could not, so a resilience proof against a gRPC dependency had no verb. The L7
---      byte faults (corrupt/throttle) stay socket-only by design, but latency/drop/after are
---      transport-generic and belong here.
---   2. A universal `.endpoint`. The doc promises "a Mock's endpoint and a Driver's target are the
---      same value" — but it is spelled `.url` on http/grpc/websocket and `.addr` on socket. Every
---      terminate/interpose surface exposes `.endpoint`: the EXACT string its driver consumes, one
---      name, so "point the driver at mock.endpoint" is uniform.
---   3. `:close()` everywhere. A proxy is a connection-shaped thing; `:close()` is its natural
---      teardown verb, and it must exist on every proxy (shell.proxy had only `:stop()`).

-- ── 1. grpc.proxy speaks the fault vocabulary ──────────────────────────────────────────────────

prova.test("grpc.proxy injects latency — a resilience verb against a gRPC dependency",
  { proves = "closing/cohesion: grpc.proxy speaks the fault vocabulary — latency, a resilience verb" }, function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/p.proto", [[
syntax = "proto3";
package p;
import "google/protobuf/empty.proto";
service P { rpc Ping (google.protobuf.Empty) returns (Pong); }
message Pong { string note = 1; }
]])
  local up = grpc.mock(t, { proto = dir .. "/p.proto" })
  up:on({ method = "p.P/Ping" }):reply({ response = { note = "pong" } })

  local proxy = grpc.proxy(t, { upstream = up.host .. ":" .. up.port })   -- passthrough
  local client = grpc.client(proxy.host .. ":" .. proxy.port)
  t:expect(client:call("p.P/Ping", {}).note):equals("pong")   -- baseline through the proxy

  proxy:latency("2s")
  local slow = grpc.client(proxy.host .. ":" .. proxy.port, { timeout = "300ms" })
  local st = slow:call_status("p.P/Ping", {})
  t:expect(st.ok):is_false()                                  -- the fault is observable
end)

-- ── 2. a universal .endpoint ────────────────────────────────────────────────────────────────────

prova.test(".endpoint is the driver-target string on every addressable mock",
  { proves = "closing/cohesion: .endpoint is the one driver-target name across .url/.addr transports" }, function(t)
  local hm = http.mock(t)
  t:expect(hm.endpoint):equals(hm.url)                        -- http: the url

  local sm = socket.mock(t, { framing = "line" })
  t:expect(sm.endpoint):equals(sm.addr)                       -- socket: the addr

  local wm = websocket.mock(t)
  t:expect(wm.endpoint):equals(wm.url)                        -- websocket: the ws url

  local dir = t:tempdir()
  fs.write(dir .. "/e.proto", [[
syntax = "proto3";
package e;
import "google/protobuf/empty.proto";
service E { rpc Go (google.protobuf.Empty) returns (google.protobuf.Empty); }
]])
  local gm = grpc.mock(t, { proto = dir .. "/e.proto" })
  t:expect(gm.endpoint):equals(gm.host .. ":" .. gm.port)     -- grpc: what grpc.client takes
end)

-- ── 3. :close() everywhere ──────────────────────────────────────────────────────────────────────

prova.test("every proxy tears down with :close() — the connection-shaped verb",
  { requires = { "unix" }, proves = "closing/cohesion: :close() tears down every proxy — the connection-shaped verb" }, function(t)
  -- shell.proxy was the odd one out (only :stop()). :close() must work identically.
  local shim = shell.proxy(t, { as = "noop", upstream = "/bin/echo" })
  shell.run("noop hi", { env = shim.env })
  t:expect(type(shim.close)):equals("function")
  shim:close()   -- must not raise; equivalent to :stop()
end)

-- ── 4. the Session contract: drivers converse in ONE vocabulary ─────────────────────────────────
--
-- Added 2026-08-18 with `stdio` (docs/plans/stdio-transport.md §3). Transports differ in how you
-- OBTAIN a stream and how you OBSERVE it; they must not differ in how you CONVERSE. Before this,
-- the four drivers disagreed about the context argument (two took none and leaked their fd to GC),
-- about `where`, and about `codec` — the kind of drift that reads as four dialects rather than one
-- API, and that a fifth transport would inherit by copying whichever one it happened to look at.

prova.test("every driver session speaks send + a bounded observe + a teardown", {
  requires = { "unix" },
  proves = "closing/cohesion: one conversation vocabulary across socket, websocket, stdio, terminal",
}, function(t)
  local m = socket.mock(t, { framing = "line" })
  m:on("ping"):reply("pong")

  local sessions = {
    socket    = socket.connect(t, { addr = m.addr, framing = "line" }),
    stdio     = stdio.spawn(t, { cmd = { "cat" }, framing = "line" }),
    terminal  = terminal.spawn(t, { cmd = { "cat" } }),
  }
  local wm = websocket.mock(t)
  sessions.websocket = websocket.connect(t, { url = wm.url })

  for name, s in pairs(sessions) do
    t:expect(type(s.send), name .. " drives with :send"):equals("function")
    -- Observe: `recv` where there are turns, `expect` where there is a stream to scan. Every
    -- session has at least one, and both are bounded — this is the anti-sleep rule made checkable.
    t:expect(type(s.recv) == "function" or type(s.expect) == "function",
      name .. " observes with :recv or :expect"):is_true()
    t:expect(type(s.close) == "function" or type(s.stop) == "function",
      name .. " tears down with :close or :stop"):is_true()
  end
end)

prova.test("every driver that holds an OS resource takes ctx first and is closed with the scope", {
  requires = { "unix" },
  proves = "closing/cohesion: a driver's fd is scope-managed, not GC-managed — the harmonization",
}, function(t)
  -- The retired positional spellings refuse, and the refusal TEACHES the new one rather than
  -- failing as "expected the test context, got a string". A first-argument change is invisible to
  -- the closed-opts gate, so this is the only place it can be caught.
  local ok, e = pcall(function() return socket.connect("tcp://127.0.0.1:1") end)
  t:expect(ok):is_false()
  t:expect(tostring(e), "socket.connect teaches the ctx-first spelling"):contains("socket.connect(ctx,")
  t:expect(tostring(e)):contains("addr =")

  local ok2, e2 = pcall(function() return websocket.connect("ws://127.0.0.1:1") end)
  t:expect(ok2):is_false()
  t:expect(tostring(e2), "websocket.connect teaches its own"):contains("url =")

  -- And a driver with no context is refused by the shared `manage` gate — the same message every
  -- mock and proxy has always given.
  local ok3, e3 = pcall(function() return stdio.spawn(nil, { cmd = { "cat" } }) end)
  t:expect(ok3):is_false()
  t:expect(tostring(e3)):contains("torn down with the scope")
end)

prova.test("`where` and `codec` behave identically wherever turns exist", {
  requires = { "unix" },
  proves = "closing/cohesion: one matcher grammar for journals AND reads — api-freeze §3, a fourth surface",
}, function(t)
  -- socket: a mock that answers three turns, of which only the third is the wanted reply.
  local m = socket.mock(t, { framing = "line" })
  m:on('{"ask":1}'):reply('{"method":"progress"}')
  m:on('{"ask":2}'):reply('{"id":9,"ok":true}')

  local c = socket.connect(t, { addr = m.addr, framing = "line", codec = "json" })
  c:send({ ask = 1 })
  c:send({ ask = 2 })
  local reply = c:recv({ where = { id = 9 }, timeout = "10s" })
  t:expect(reply.ok, "socket: `where` skipped the notification"):is_true()

  -- stdio: the same grammar, the same result, over pipes instead of a socket.
  local s = stdio.spawn(t, { cmd = { "cat" }, framing = "line", codec = "json" })
  s:send({ method = "progress" })
  s:send({ id = 9, ok = true })
  t:expect(s:recv({ where = { id = 9 }, timeout = "10s" }).ok,
    "stdio: identical selector, identical outcome"):is_true()

  -- The negative control both halves need: without `where`, each returns the FIRST turn — so the
  -- assertions above measure the selector rather than the order things happened to arrive in.
  local s2 = stdio.spawn(t, { cmd = { "cat" }, framing = "line", codec = "json" })
  s2:send({ method = "progress" })
  s2:send({ id = 9, ok = true })
  t:expect(s2:recv({ timeout = "10s" }).method):equals("progress")
end)
