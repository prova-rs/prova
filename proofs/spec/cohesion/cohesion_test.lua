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
