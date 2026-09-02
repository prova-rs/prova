--- `wss://` and gRPC-over-TLS — the two transports that could not reach a TLS endpoint at all.
---
--- `websocket.connect` refused anything but `ws://` at the call (`websocket.rs`, "no TLS in v1"),
--- and `grpc` normalized every address to `http://`, so a TLS gRPC endpoint was unaddressable. Both
--- now take the same `insecure`/`ca_cert` pair as `http`, from the same parser
--- (docs/design/architecture.md#tls-everywhere).
---
--- **Why these two are proven through a nested SUITE rather than `prova eval`.** Both verbs need a
--- scope to be torn down with (`websocket.connect(ctx, …)`), and `eval` deliberately has none — it
--- refuses with *"pass the test or fixture context"*. So the subject here is a real proof file,
--- scaffolded into a temp package and run by `prova.bin`: the feature still executes in the tree
--- under test, and the assertion is made on that run's exit code and output.
---
--- **The plaintext mock is infrastructure and stays in the conductor.** `websocket.mock` and
--- `grpc.mock` are what a TLS front door is put *in front of*; wrapping them in TLS is not what is
--- being proven, reaching them over TLS is. This is the same split `tls_test.lua` makes with its
--- python service.
---
--- Gated on `openssl` and `python3`.

local scaffold = require("scaffold")
local tlspki = require("tlspki")

local GATES = { "openssl", "python3" }

--- The port out of a `ws://127.0.0.1:<port>` / `http://127.0.0.1:<port>` mock URL.
---
--- Parsed rather than taken from a field because `WsMock` exposes only `url` — and a mock's port is
--- random by design, so a proof that hardcoded one would collide with a second run on the machine.
local function port_of(url)
  -- Parenthesised: `assert` returns its message alongside its value, and an extra return value
  -- reaching `tonumber` silently becomes the BASE argument ("number expected, got string").
  return tonumber((assert(url:match(":(%d+)$"), "no port in " .. tostring(url))))
end

--- Run a one-test proof in a scaffolded package, through `prova.bin`. Returns the shell result.
---
--- `must_pass` is left to the caller: a nested run is asserted on its EXIT CODE, and half of what
--- is worth proving here is a nested run that correctly fails.
local function nested(t, body)
  local proj = scaffold.package(t, {
    name = "tls-subject",
    proofs = { ["subject_test.lua"] = body },
  })
  return shell.run({ prova.bin }, { cwd = proj, merge_stderr = true, timeout = "120s" })
end

prova.test("websocket.connect reaches a wss:// endpoint, verifying a private CA", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the transport that hard-REFUSED tls at the call site — a rejection at the argument check, not a missing capability — so nothing about it could work until the check itself changed, and a message turn completing over the TLS leg is what shows the connector was wired rather than the guard merely deleted",
  requires = GATES,
}, function(t)
  local pki = tlspki.mint(t, "ws-pki")

  -- The plaintext ws server, in the conductor: this is the dependency being fronted.
  local m = websocket.mock(t)
  m:on("ping"):reply("pong")

  local front = tlspki.terminate(t, t, { pki = pki, upstream_port = port_of(m.url) })

  local r = nested(t, string.format([[
prova.test("a wss round trip", {}, function(t)
  local c = websocket.connect(t, { url = %q, ca_cert = %q })
  c:send("ping")
  t:expect(c:recv()):equals("pong")
end)
]], front.ws_url, pki.ca))

  t:expect(r.code, "the nested suite is green:\n" .. r.stdout):equals(0)
  -- The mock's own journal is the independent witness: the conductor's server saw the turn, so the
  -- bytes really crossed the TLS leg rather than the subject asserting against itself.
  t:expect(#m:received({ data = "ping" }), "the fronted mock saw the message"):equals(1)
end)

prova.test("a wss:// endpoint whose CA is unknown is refused", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the negative control for the test above: `wss://` must not mean 'TLS, unverified' — a connection that succeeded with no `ca_cert` would mean the scheme was accepted and the verification skipped, which is strictly worse than the ws-only refusal it replaced",
  requires = GATES,
}, function(t)
  local pki = tlspki.mint(t, "ws-pki-neg")
  local m = websocket.mock(t)
  m:on("ping"):reply("pong")
  local front = tlspki.terminate(t, t, { pki = pki, upstream_port = port_of(m.url) })

  local r = nested(t, string.format([[
prova.test("a wss connection with default roots", {}, function(t)
  websocket.connect(t, { url = %q })
end)
]], front.ws_url))

  t:expect(r.code, "the nested suite FAILS:\n" .. r.stdout):never():equals(0)
end)

--- `insecure` on `wss://` and on `grpc` exercises a DIFFERENT mechanism than it does on `http`,
--- and that is why these two tests exist.
---
--- reqwest has its own `danger_accept_invalid_certs`, so the http-side `insecure` proof never
--- touches prova's hand-written accept-any verifier. websocket and grpc have no such switch — they
--- take a `rustls::ClientConfig` / a `ServerCertVerifier` object, which prova supplies. Mutation
--- testing is what surfaced the gap: breaking `verify_server_cert` to reject everything left the
--- whole suite green, because nothing reached it. These are the tests that now fail.
prova.test("insecure = true reaches a wss:// endpoint nothing trusts", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the accept-any verifier prova writes by hand is only reachable through websocket and grpc — reqwest brings its own — so without this the forty lines that decide whether a certificate is checked had no test at all, which mutation testing proved by breaking them silently",
  requires = GATES,
}, function(t)
  local pki = tlspki.mint(t, "ws-pki-insecure")
  local m = websocket.mock(t)
  m:on("ping"):reply("pong")
  local front = tlspki.terminate(t, t, { pki = pki, upstream_port = port_of(m.url) })

  local r = nested(t, string.format([[
prova.test("a wss round trip with insecure", {}, function(t)
  local c = websocket.connect(t, { url = %q, insecure = true })
  c:send("ping")
  t:expect(c:recv()):equals("pong")
end)
]], front.ws_url))

  t:expect(r.code, "the nested suite is green:\n" .. r.stdout):equals(0)
  t:expect(#m:received({ data = "ping" }), "the turn crossed the unverified TLS leg"):equals(1)
end)

prova.test("insecure = true reaches a grpc endpoint nothing trusts", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "grpc's insecure path is a third mechanism again — tonic's `tls_config_with_verifier`, which REPLACES the root store rather than adding to it — so it can fail independently of both the http and websocket paths while they stay green",
  requires = GATES,
}, function(t)
  local pki = tlspki.mint(t, "grpc-pki-insecure")
  local proto = pki.dir .. "/greeter.proto"
  fs.write(proto, [[
syntax = "proto3";
package tlsdemo;
message Req { string name = 1; }
message Rep { string greeting = 1; }
service Greeter { rpc Hello(Req) returns (Rep); }
]])
  local m = grpc.mock(t, { proto = proto })
  m:on({ method = "tlsdemo.Greeter/Hello" }):reply({ response = { greeting = "hi unverified" } })
  local front = tlspki.terminate(t, t, {
    pki = pki,
    upstream_port = port_of(m.url),
    alpn = "h2",
  })

  local r = nested(t, string.format([[
prova.test("a grpc call over insecure tls", {}, function(t)
  local c = grpc.client(%q, { insecure = true })
  t:expect(c:call("tlsdemo.Greeter/Hello", { name = "prova" }).greeting):equals("hi unverified")
end)
]], front.url))

  t:expect(r.code, "the nested suite is green:\n" .. r.stdout):equals(0)
end)

prova.test("websocket.connect still refuses a scheme that is neither ws nor wss", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "widening a guard is where guards get deleted: `wss://` had to be admitted without admitting `http://` or a bare host:port, and the refusal has to keep naming both accepted spellings or the author's next guess is a coin flip",
  requires = GATES,
}, function(t)
  local r = shell.run({ prova.bin, "eval",
    'local ok, e = pcall(function() return websocket.connect(nil, { url = "http://x/" }) end) ' ..
    'return tostring(ok) .. " " .. tostring(e)' },
    { merge_stderr = true, timeout = "60s" })

  t:expect(r.stdout:find("ws:// or wss://", 1, true) ~= nil,
    "the refusal names both accepted schemes: " .. r.stdout):equals(true)
end)

prova.test("grpc reaches a TLS endpoint, verifying a private CA", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "gRPC over TLS is not just 'set a flag': the channel URI has to become https, tonic's ClientTlsConfig has to carry the anchors, and ALPN has to negotiate h2 — a completed reflection handshake plus a decoded unary reply is the only assertion that exercises all three at once",
  requires = GATES,
}, function(t)
  local pki = tlspki.mint(t, "grpc-pki")

  -- A plaintext gRPC mock in the conductor, serving reflection so the subject's client needs no
  -- .proto — exactly how `grpc.client` reaches any real server.
  local proto = pki.dir .. "/greeter.proto"
  fs.write(proto, [[
syntax = "proto3";
package tlsdemo;
message Req { string name = 1; }
message Rep { string greeting = 1; }
service Greeter { rpc Hello(Req) returns (Rep); }
]])
  local m = grpc.mock(t, { proto = proto })
  m:on({ method = "tlsdemo.Greeter/Hello" }):reply({ response = { greeting = "hi over tls" } })

  -- ALPN "h2" because gRPC is HTTP/2 and h2 over TLS is NEGOTIATED, not assumed: a terminator that
  -- did not offer it would fail the handshake, which is the failure this argument exists to avoid.
  local front = tlspki.terminate(t, t, {
    pki = pki,
    upstream_port = port_of(m.url),
    alpn = "h2",
  })

  local r = nested(t, string.format([[
prova.test("a grpc call over tls", {}, function(t)
  local c = grpc.client(%q, { ca_cert = %q })
  local rep = c:call("tlsdemo.Greeter/Hello", { name = "prova" })
  t:expect(rep.greeting):equals("hi over tls")
end)
]], front.url, pki.ca))

  t:expect(r.code, "the nested suite is green:\n" .. r.stdout):equals(0)
end)

prova.test("grpc promotes a bare host:port only when asked, and refuses a contradiction", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "grpc has no scheme in its most common address form, so TLS needs a spelling that is not a URL — and the risk of admitting three spellings is that a fourth, contradictory one (`http://` with a TLS option) silently picks a winner the author cannot predict, with the loser being whether traffic was encrypted",
  requires = GATES,
}, function(t)
  local r = shell.run({ prova.bin, "eval",
    'local ok, e = pcall(function() return grpc.client("http://localhost:1", { insecure = true }) end) ' ..
    'return tostring(ok) .. " " .. tostring(e)' },
    { merge_stderr = true, timeout = "60s" })

  t:expect(r.stdout:find("false", 1, true) ~= nil,
    "the contradiction is refused: " .. r.stdout):equals(true)
  t:expect(r.stdout:find("http://", 1, true) ~= nil,
    "…naming the address it read: " .. r.stdout):equals(true)
  t:expect(r.stdout:find("https://", 1, true) ~= nil,
    "…and the spelling that would have worked: " .. r.stdout):equals(true)
end)
