--- Generalized cassette redaction (docs/design/mocks-proxies-drivers.md) — recording real traffic
--- writes real traffic to a file someone will commit, so scrubbing secrets at RECORD TIME is a
--- kernel facility, not an http-only convenience. Today only `http.proxy` redacts; a recorded gRPC
--- cassette can carry an auth token in a request field and a shell cassette can capture a secret in
--- stdout, both hitting disk unredacted. This closes that asymmetry.
---
--- The cross-transport contract: `redact = { "secret", … }` — literal strings the engine replaces
--- with a sentinel in the SERIALIZED cassette before it is written, regardless of transport. A
--- transport may keep richer conveniences on top (http's redact-by-header-name), but the floor is
--- "these strings never touch disk." The in-memory journal is NOT redacted — that is where a test
--- asserts the secret was actually sent.

-- ── grpc ─────────────────────────────────────────────────────────────────────────────────────

prova.test("a grpc cassette redacts a named secret from the recorded request",
  { proves = "closing/redaction: a grpc cassette scrubs named secrets from the recorded request" }, function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/echo.proto", [[
syntax = "proto3";
package echo;
service Echo { rpc Say (Msg) returns (Msg); }
message Msg { string note = 1; }
]])
  local up = grpc.mock(t, { proto = dir .. "/echo.proto" })
  up:on({ method = "echo.Echo/Say" }):reply({ response = { note = "ok" } })

  local cas = dir .. "/echo.cassette"
  local p = grpc.proxy(t, {
    upstream = up.host .. ":" .. up.port, cassette = cas, mode = "record",
    redact = { "sk-live-abc123" },
  })
  grpc.client(p.host .. ":" .. p.port):call("echo.Echo/Say", { note = "sk-live-abc123" })
  p:close()

  t:expect(fs.read(cas)):never():contains("sk-live-abc123")   -- scrubbed before disk
end)

-- ── socket ───────────────────────────────────────────────────────────────────────────────────

prova.test("a socket cassette redacts a secret from a recorded turn",
  { proves = "closing/redaction: a socket cassette scrubs a secret from a recorded turn" }, function(t)
  local srv = socket.mock(t, { framing = "line" })
  srv:on("AUTH sk-live-xyz"):reply("OK")

  local cas = t:tempdir() .. "/socket.cassette"
  local p = socket.proxy(t, {
    upstream = srv.addr, framing = "line", cassette = cas, mode = "record",
    redact = { "sk-live-xyz" },
  })
  local c = socket.connect(t, { addr = p.addr, framing = "line" })
  c:send("AUTH sk-live-xyz")
  t:expect(c:recv()):equals("OK")
  p:close()

  t:expect(fs.read(cas)):never():contains("sk-live-xyz")
end)

-- ── shell ────────────────────────────────────────────────────────────────────────────────────

prova.test("a shell cassette redacts a secret captured in stdout",
  { requires = { "unix" }, proves = "closing/redaction: a shell cassette scrubs a secret captured in stdout" }, function(t)
  local cli = t:tempdir() .. "/tokened"
  fs.write(cli, "#!/bin/sh\nprintf 'token=sk-live-shh\\n'\n")
  shell.run({ "chmod", "+x", cli }, { check = true })

  local cas = t:tempdir() .. "/shell.cassette"
  local shim = shell.proxy(t, {
    as = "getcreds", upstream = cli, cassette = cas, mode = "record",
    redact = { "sk-live-shh" },
  })
  shell.run("getcreds", { env = shim.env })
  shim:stop()

  t:expect(fs.read(cas)):never():contains("sk-live-shh")
end)

-- ── the floor holds everywhere the http convenience already did ────────────────────────────────

prova.test("http keeps its by-header-name redaction AND honors literal `redact` strings",
  { proves = "closing/redaction: http honors literal redact strings on top of by-header-name" }, function(t)
  local up = http.mock(t)
  up:on{ path = "/x" }:reply{ status = 200, json = { echoed = true } }

  local cas = t:tempdir() .. "/http.cassette"
  local p = http.proxy(t, {
    upstream = up.url, cassette = cas, mode = "record",
    redact = { "tenant-acme-42" },   -- a literal string, not a header name
  })
  http.get(p.url .. "/x?tenant=tenant-acme-42")
  p:close()

  t:expect(fs.read(cas)):never():contains("tenant-acme-42")
end)
