--- grpc cassettes — the second specialization of the kernel record/replay engine
--- (docs/design/mocks-proxies-drivers.md; http.proxy was the first). Decisions these specs pin:
---
---   * `grpc.proxy(ctx, { upstream, cassette?, mode? })` — the same verb and modes as http:
---     passthrough (default) | record | replay | auto. Same object model as the http face:
---     a proxy is the mock's dial, not a second concept.
---   * Turn model: one unary call. Match key: full method + the request message (structural),
---     so replay distinguishes payloads, not just methods.
---   * THE CASSETTE CARRIES THE DESCRIPTORS. Record mode captures the schema the upstream
---     served over reflection, so a replay proxy needs no proto and no upstream — the cassette
---     is a complete, self-describing fake. This is what "a proxy in record mode manufactures
---     a mock" means for a schema'd protocol.
---   * A replay miss is LOUD: status `Unavailable` (the 502 analog — the recording
---     infrastructure failed the call, not the service), message naming the cassette.

local PROTO = [[
syntax = "proto3";
package ping;
import "google/protobuf/empty.proto";
service Ping {
  rpc Poke (google.protobuf.Empty) returns (Pong);
  rpc Echo (Pong) returns (Pong);
}
message Pong {
  string note = 1;
}
]]

local function ping_upstream(t)
  local dir = t:tempdir()
  fs.write(dir .. "/ping.proto", PROTO)
  local m = grpc.mock(t, { proto = dir .. "/ping.proto" })
  m:on({ method = "ping.Ping/Poke" }):reply({ response = { note = "poked" } })
  m:on({ method = "ping.Ping/Echo" }):reply({ response = { note = "echoed" } })
  return m
end

prova.test("record mode captures calls while traffic flows; close is the flush point",
  { proves = "tier-a/grpc-cassettes: record captures calls while traffic flows; close is the flush point" }, function(t)
  local up = ping_upstream(t)
  local cas = t:tempdir() .. "/poke.cassette"

  local p = grpc.proxy(t, { upstream = up.host .. ":" .. up.port, cassette = cas, mode = "record" })
  local client = grpc.client(p.host .. ":" .. p.port)
  t:expect(client:call("ping.Ping/Poke", {}).note):equals("poked")   -- flows while recording
  p:close()

  t:expect(fs.exists(cas)):is_true()
end)

prova.test("replay needs no upstream and no proto — the cassette carries the descriptors",
  { proves = "tier-a/grpc-cassettes: the cassette carries the descriptors — replay needs no proto/upstream" }, function(t)
  local up = ping_upstream(t)
  local cas = t:tempdir() .. "/schema.cassette"

  local rec = grpc.proxy(t, { upstream = up.host .. ":" .. up.port, cassette = cas, mode = "record" })
  t:expect(grpc.client(rec.host .. ":" .. rec.port):call("ping.Ping/Poke", {}).note):equals("poked")
  rec:close()
  up:stop()                                        -- reality leaves the building

  local rep = grpc.proxy(t, { cassette = cas, mode = "replay" })   -- no upstream, no proto
  local client = grpc.client(rep.host .. ":" .. rep.port)          -- reflection FROM the cassette
  t:expect(client:call("ping.Ping/Poke", {}).note):equals("poked")
end)

prova.test("the match key is method + request message — a different payload is a loud miss",
  { proves = "tier-a/grpc-cassettes: match key is method+request; a miss is Unavailable naming the cassette" }, function(t)
  local up = ping_upstream(t)
  local cas = t:tempdir() .. "/miss.cassette"

  local rec = grpc.proxy(t, { upstream = up.host .. ":" .. up.port, cassette = cas, mode = "record" })
  grpc.client(rec.host .. ":" .. rec.port):call("ping.Ping/Echo", { note = "recorded" })
  rec:close()
  up:stop()

  local rep = grpc.proxy(t, { cassette = cas, mode = "replay" })
  local client = grpc.client(rep.host .. ":" .. rep.port)
  t:expect(client:call("ping.Ping/Echo", { note = "recorded" }).note):equals("echoed")

  local st = client:call_status("ping.Ping/Echo", { note = "never-recorded" })
  t:expect(st.code):equals("Unavailable")          -- infrastructure failing, not the service
  t:expect(st.message):contains("cassette")
end)

prova.test("passthrough is the plain dial — forward, record nothing",
  { proves = "tier-a/grpc-cassettes: passthrough is the plain dial — forward, record nothing" }, function(t)
  local up = ping_upstream(t)
  local cas = t:tempdir() .. "/untouched.cassette"

  local p = grpc.proxy(t, { upstream = up.host .. ":" .. up.port, cassette = cas, mode = "passthrough" })
  t:expect(grpc.client(p.host .. ":" .. p.port):call("ping.Ping/Poke", {}).note):equals("poked")
  p:close()

  t:expect(fs.exists(cas)):is_false()
end)
