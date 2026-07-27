--- One `received()` vocabulary (docs/plans/api-freeze.md §6). Every observation journal —
--- http.mock, grpc.mock, prova.double, and every future transport mock — records entries
--- carrying the same spine:
---
---   seq      monotonic per mock, 1-based — call ordering falls out of the journal
---   source   "stub" | "target" | "unmatched" (a transport may add its own sources)
---   matched  bool — did a stub answer this?
---
--- plus the transport-native payload fields (http keeps method/path/query/headers/body/params/
--- status; grpc keeps method/request/code). Filters accept the same subset-matcher shapes as
--- `:on` — a table is a subset match, a function is a predicate.
---
--- `prova.double` is the shipped reference implementation of exactly this shape (see
--- crates/prova-core/src/plugins/prova/double.lua and proofs/doubles/); implementing means
--- converging the two server mocks on it, not inventing a new model.

-- ── http.mock ────────────────────────────────────────────────────────────────────────────────

prova.test("http.mock journal entries carry seq/source/matched over the http-native fields",
  { spec = "api-freeze §6: journal spine on http.mock — not built" }, function(t)
  local m = http.mock(t)
  m:on{ method = "GET", path = "/a" }:reply{ status = 200 }

  http.get(m.url .. "/a")
  http.get(m.url .. "/a")

  local e = m:received()
  t:expect(e):has_length(2)
  t:expect(e[1].seq):equals(1)                 -- monotonic, 1-based, per mock
  t:expect(e[2].seq):equals(2)
  t:expect(e[1].source):equals("stub")
  t:expect(e[1].matched):is_true()
  t:expect(e[1].method):equals("GET")          -- transport-native fields are kept, not replaced
  t:expect(e[1].path):equals("/a")
end)

prova.test("an unmatched http request is journaled too — matched=false, source=unmatched",
  { spec = "api-freeze §6: unmatched entries journaled — not built" }, function(t)
  local m = http.mock(t)
  m:on{ path = "/known" }:reply{ status = 200 }

  http.get(m.url .. "/known")
  http.get(m.url .. "/unknown")

  local u = m:received{ matched = false }      -- the filter is the same subset matcher as :on
  t:expect(u):has_length(1)
  t:expect(u[1].source):equals("unmatched")
  t:expect(u[1].path):equals("/unknown")
end)

prova.test("received() filters accept a predicate function, exactly like :on",
  { spec = "api-freeze §6: predicate filters — not built" }, function(t)
  local m = http.mock(t)
  m:on{ path = "/n" }:reply{ status = 200 }
  http.get(m.url .. "/n")
  http.get(m.url .. "/n")

  local second = m:received(function(entry) return entry.seq == 2 end)
  t:expect(second):has_length(1)
  t:expect(second[1].seq):equals(2)
end)

-- ── grpc.mock ────────────────────────────────────────────────────────────────────────────────

prova.test("grpc.mock journal entries carry the same spine over the grpc-native fields",
  { spec = "api-freeze §6: journal spine on grpc.mock — not built" }, function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/ping.proto", [[
syntax = "proto3";
package ping;
import "google/protobuf/empty.proto";
service Ping {
  rpc Poke (google.protobuf.Empty) returns (Pong);
}
message Pong {
  string note = 1;
}
]])
  local m = grpc.mock(t, { proto = dir .. "/ping.proto" })
  m:on({ method = "ping.Ping/Poke" }):reply({ response = { note = "poked" } })

  local client = grpc.client(m.host .. ":" .. m.port)
  client:call("ping.Ping/Poke", {})
  client:call("ping.Ping/Poke", {})

  local e = m:received()
  t:expect(e):has_length(2)
  t:expect(e[1].seq):equals(1)
  t:expect(e[2].seq):equals(2)
  t:expect(e[1].source):equals("stub")
  t:expect(e[1].matched):is_true()
  t:expect(e[1].method):equals("ping.Ping/Poke")   -- grpc-native fields are kept
end)
