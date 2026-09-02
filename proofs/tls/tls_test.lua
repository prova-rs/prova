--- Every network client speaks TLS, and they all take the same two options.
---
--- Before this, `http.get("https://…")` failed with reqwest's *"invalid URL, scheme is not http"* —
--- an error naming neither TLS nor the way out — `websocket.connect` rejected anything but `ws://`,
--- and `grpc` had no way to reach a TLS endpoint at all. The stance was deliberate and right for
--- the local-container mission; it stopped being right the moment a real system under test sat
--- behind a certificate.
---
--- **The negative controls are the point of this file.** "TLS works" is satisfied by a client that
--- accepts every certificate, so every positive assertion here is paired with one that must FAIL:
--- verified TLS against a self-signed leaf is refused, `ca_cert` pointed at an unrelated CA is
--- refused, and `insecure` is shown to change the outcome rather than the code path. Without those,
--- a `danger_accept_invalid_certs(true)` left on by accident would read as a green suite.
---
--- **Every call under proof runs in the SUBJECT** (`prova.bin`), never in this file: an `http.get`
--- written directly in a proof body exercises whichever prova is CONDUCTING, so a TLS proof would
--- go green on the commit that broke TLS as long as the conductor had it. The PKI and the servers
--- are infrastructure, so they stay here; the feature reaches through `prova.bin eval`.
---
--- Gated on `openssl` and `python3` — the certificate authority and the TLS front end. Neither is
--- prova's subject, and a host without them skips rather than fails.

local tlspki = require("tlspki")

-- A plaintext HTTP/1.1 service, deliberately dumb: it answers /health and echoes the path as JSON,
-- which is enough for a status assertion, a `:json()` assertion and a readiness poll. It binds a
-- literal IP and overrides `server_bind` for the reason `proofs/http/probe_test.lua` documents at
-- length: stock `http.server` does a reverse-DNS lookup between bind() and listen(), which costs
-- ~70s on GitHub's macOS runners and turns readiness into a coin flip.
local SERVICE_PY = [[
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from socketserver import TCPServer

port = int(sys.argv[1])

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _json(self, body):
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self._json(b'{"tls":true,"path":"' + self.path.encode() + b'"}')
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        self.rfile.read(n)
        # Shaped as a GraphQL reply so one service can stand in for both clients.
        self._json(b'{"data":{"ok":true}}')

class S(ThreadingHTTPServer):
    daemon_threads = True
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name, self.server_port = "localhost", self.server_address[1]

srv = S(("127.0.0.1", port), H)
print("listening", flush=True)
srv.serve_forever()
]]

--- The whole TLS rig, once per file: a CA, a leaf it signed, an unrelated CA for the negative
--- control, a plaintext service, and a TLS front door in front of it.
---
--- File-scoped because minting two RSA keypairs and booting two processes costs more than every
--- assertion below combined, and nothing here is mutated by a test.
local rig = prova.fixture("tls rig", Scope.File, function(ctx)
  local pki = tlspki.mint(ctx, "pki")
  local other_ca = tlspki.other_ca(ctx, "other")

  local python = package.config:sub(1, 1) == "\\" and "python" or "python3"
  local script = pki.dir .. "/service.py"
  fs.write(script, SERVICE_PY)
  local plain_port = net.free_port()
  local proc = ctx:manage(shell.spawn({ python, "-u", script, tostring(plain_port) }))
  for _ = 1, 600 do
    if (proc:output() or ""):find("listening", 1, true) then break end
    if not proc:running() then
      error("the plaintext service exited before listening: " .. (proc:output() or ""), 0)
    end
    prova.sleep(50)
  end

  local front = tlspki.terminate(ctx, ctx, { pki = pki, upstream_port = plain_port })
  return {
    pki = pki,
    other_ca = other_ca,
    url = front.url,
    plain_url = "http://127.0.0.1:" .. plain_port,
  }
end)

--- Run Lua in the subject and hand back the raw result, so an assertion can be made on the far
--- side of the process boundary — including on a FAILURE, which half of these tests are about.
local function eval(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "60s" })
end

local GATES = { "openssl", "python3" }

prova.test("a verified https request reaches a service behind a private CA", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the positive case has to be a VERIFIED one — `insecure` passing proves only that verification can be switched off, while a request that validates a real chain to a real CA proves the trust path actually works",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
local res = http.get(%q, { ca_cert = %q })
return res.status .. " " .. res:json().path
]==], r.url .. "/health", r.pki.ca))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  local status, path = out.stdout:match("(%d+) (%S+)")
  t:expect(status, "the https GET succeeds"):equals("200")
  t:expect(path, "…and the body decoded, so the whole exchange was intact"):equals("/health")
end)

prova.test("the same request with no ca_cert is refused", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the negative control for the test above: a client trusting only the default roots MUST reject a private chain, otherwise `ca_cert` is decoration and the verified case above proves nothing",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
local ok, e = pcall(function() return http.get(%q) end)
return tostring(ok) .. " " .. tostring(e)
]==], r.url .. "/health"))

  t:expect(out.stdout:find("false", 1, true) ~= nil,
    "the request fails: " .. out.stdout):equals(true)
  t:expect(out.stdout:lower():find("certificate", 1, true) ~= nil,
    "…and says it was the certificate, not a connection problem: " .. out.stdout):equals(true)
end)

prova.test("ca_cert pointed at an unrelated CA is refused", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the second negative control: `ca_cert` must ADD a specific anchor, not merely turn verification off — a wrong-CA request that succeeded would mean the option's value was never read",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
local ok, e = pcall(function() return http.get(%q, { ca_cert = %q }) end)
return tostring(ok) .. " " .. tostring(e)
]==], r.url .. "/health", r.other_ca))

  t:expect(out.stdout:find("false", 1, true) ~= nil,
    "the request fails: " .. out.stdout):equals(true)
end)

prova.test("insecure = true reaches a certificate nothing trusts", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the boot-then-probe case: a service prova just started with a self-signed certificate is the runner's most common TLS subject, and it is unreachable under verified-only TLS — the same URL that fails above must succeed here, which is what makes this a measurement of the option rather than of the server",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
return http.get(%q, { insecure = true }).status
]==], r.url .. "/health"))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  t:expect(out.stdout:match("%d+"), "the insecure GET succeeds"):equals("200")
end)

prova.test("a client declares the policy once, for every call it makes", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the ergonomic that keeps `insecure` from being repeated per call — repetition only has to be missed once to produce a handshake error instead of a result, and a suite pointed at one self-signed service makes that call dozens of times",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
local c = http.client{ base_url = %q, ca_cert = %q }
return c:get("/health").status .. " " .. c:get("/other").status
]==], r.url, r.pki.ca))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  t:expect(out.stdout:match("(%d+) (%d+)") and out.stdout:match("%d+ (%d+)"),
    "both calls inherited the client's CA"):equals("200")
end)

prova.test("wait_for polls an https endpoint", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "readiness is the FIRST thing to touch a service prova just booted, so a poll that could not carry a certificate policy would make boot-then-probe the one place TLS stops working — the exact shape of gap this change exists to close",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
return http.wait_for(%q, { ca_cert = %q, timeout = "20s" }).status
]==], r.url .. "/health", r.pki.ca))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  t:expect(out.stdout:match("%d+"), "the poll succeeded over TLS"):equals("200")
end)

prova.test("graphql reaches an https endpoint", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "graphql shares reqwest with `http` but built its own client, so it was the place a TLS policy could silently not apply — sharing a dependency is not the same as sharing behavior",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
local c = graphql.client{ url = %q, ca_cert = %q }
local d = c:query("{ ok }")
return tostring(d.ok)
]==], r.url .. "/graphql", r.pki.ca))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  t:expect(out.stdout:find("true", 1, true) ~= nil,
    "the graphql query round-tripped over TLS: " .. out.stdout):equals(true)
end)

--- The six call sites that accept the pair, asserted in ONE test rather than six.
---
--- "They all refuse identically" is a single property about the set, and splitting it per site
--- would let five stay green while the sixth quietly stopped refusing — which is the exact failure
--- this asserts against. The addresses are unroutable on purpose: every one of these must fail
--- during option parsing, before anything is dialed, so a test that needed a live server would be
--- measuring the wrong thing.
local CONTRADICTIONS = {
  { name = "http.get", code = 'http.get("https://localhost:1/", { insecure = true, ca_cert = "/tmp/ca.pem" })' },
  { name = "http.client", code = 'http.client{ base_url = "https://localhost:1", insecure = true, ca_cert = "/tmp/ca.pem" }' },
  { name = "http.wait_for", code = 'http.wait_for("https://localhost:1/", { insecure = true, ca_cert = "/tmp/ca.pem" })' },
  { name = "graphql.client", code = 'graphql.client{ url = "https://localhost:1", insecure = true, ca_cert = "/tmp/ca.pem" }' },
  { name = "websocket.connect", code = 'websocket.connect(nil, { url = "wss://localhost:1", insecure = true, ca_cert = "/tmp/ca.pem" })' },
  { name = "grpc.client", code = 'grpc.client("https://localhost:1", { insecure = true, ca_cert = "/tmp/ca.pem" })' },
}

prova.test("insecure with ca_cert is refused identically on every client", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the two options are contradictory intents, and one silently winning is how a proof keeps passing after its pinning stops meaning anything — refusing at five call sites and accepting at the sixth would be worse than not refusing at all, because the exception is the one nobody documents",
}, function(t)
  for _, case in ipairs(CONTRADICTIONS) do
    local out = eval("local ok, e = pcall(function() return " .. case.code ..
                     " end) return tostring(ok) .. ' ' .. tostring(e)")

    t:expect(out.stdout:find("false", 1, true) ~= nil,
      case.name .. " refuses the pair: " .. out.stdout):equals(true)
    t:expect(out.stdout:find("contradictory", 1, true) ~= nil,
      case.name .. " says why: " .. out.stdout):equals(true)
    t:expect(out.stdout:find("Name exactly one", 1, true) ~= nil,
      case.name .. " says what to do instead: " .. out.stdout):equals(true)
  end
end)

prova.test("a ca_cert that is not a certificate is refused before the handshake", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "a PEM that parses to no anchors is a wrong-file mistake — a key, a CSR, a truncated download — and accepting it as 'no extra anchors' would leave the connection trusting only the default roots while the proof source reads as pinned",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
local ok, e = pcall(function() return http.get(%q, { ca_cert = %q }) end)
return tostring(ok) .. " " .. tostring(e)
]==], r.url .. "/health", r.pki.key))

  t:expect(out.stdout:find("no CERTIFICATE block", 1, true) ~= nil,
    "the error names what is wrong with the FILE: " .. out.stdout):equals(true)
end)

prova.test("an unreadable ca_cert is distinguished from a ca_cert that is the wrong file", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "both mistakes name the option and the path, so asserting on THOSE cannot tell them apart — mutation testing showed exactly that, by making the file read succeed with empty bytes and watching the 'missing path' test stay green on the wrong-contents error. They need different fixes (correct the path, versus point at a different file), so the messages have to differ",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local missing = eval([==[
local ok, e = pcall(function()
  return http.get("https://localhost:1/", { ca_cert = "/nonexistent/ca.pem" })
end)
return tostring(ok) .. " " .. tostring(e)
]==])

  t:expect(missing.stdout:find("ca_cert", 1, true) ~= nil,
    "names the option: " .. missing.stdout):equals(true)
  t:expect(missing.stdout:find("/nonexistent/ca.pem", 1, true) ~= nil,
    "…and the path: " .. missing.stdout):equals(true)
  -- The discriminator: an unreadable file is a READ failure and must say so, never "no
  -- CERTIFICATE block" — which would send the author looking inside a file that is not there.
  t:expect(missing.stdout:find("reading `ca_cert`", 1, true) ~= nil,
    "…and says the file could not be READ: " .. missing.stdout):equals(true)
  t:expect(missing.stdout:find("no CERTIFICATE block", 1, true) == nil,
    "…and does NOT blame the contents of a file it never read: " .. missing.stdout):equals(true)

  -- The other side of the same distinction, so neither message can drift into the other's job.
  local wrong = eval(string.format([==[
local ok, e = pcall(function() return http.get(%q, { ca_cert = %q }) end)
return tostring(ok) .. " " .. tostring(e)
]==], r.url .. "/health", r.pki.key))

  t:expect(wrong.stdout:find("no CERTIFICATE block", 1, true) ~= nil,
    "a readable non-certificate blames the contents: " .. wrong.stdout):equals(true)
  t:expect(wrong.stdout:find("reading `ca_cert`", 1, true) == nil,
    "…and not the read: " .. wrong.stdout):equals(true)
end)

prova.test("plaintext http is untouched by any of this", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the regression control: TLS arrived as a new branch through `send`, and the failure mode nobody would notice quickly is the ordinary `http://` request getting slower, different, or broken — every other proof in this tree depends on it staying exactly what it was",
  requires = GATES,
}, function(t)
  local r = t:use(rig)
  local out = eval(string.format([==[
return http.get(%q).status
]==], r.plain_url .. "/health"))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  t:expect(out.stdout:match("%d+"), "a plaintext GET still works, with no options at all"):equals("200")
end)

--- `insecure` and `ca_cert` disagreeing about a HOSTNAME is the drift that would be hardest to see.
---
--- A certificate can be perfectly signed by a CA you trust and still be the wrong certificate for
--- the host you dialed — the name is not the chain. Each transport checks it somewhere different:
--- reqwest has a `danger_accept_invalid_hostnames` knob SEPARATE from
--- `danger_accept_invalid_certs`, while prova's own verifier ignores the name outright. So "one
--- policy" could quietly have meant "http still checks the name, websocket and grpc do not," and
--- nothing above this line would have noticed.
local mismatch = prova.fixture("hostname-mismatched service", Scope.File, function(ctx)
  -- Signed by the SAME CA as `rig`, valid only for a name nothing will dial. So a failure here is
  -- unambiguously about the name: the chain is genuinely good.
  local pki = tlspki.mint(ctx, "mismatch-pki", { san = "DNS:wrong.example", cn = "wrong.example" })
  local python = package.config:sub(1, 1) == "\\" and "python" or "python3"
  local script = pki.dir .. "/service.py"
  fs.write(script, SERVICE_PY)
  local plain_port = net.free_port()
  local proc = ctx:manage(shell.spawn({ python, "-u", script, tostring(plain_port) }))
  for _ = 1, 600 do
    if (proc:output() or ""):find("listening", 1, true) then break end
    if not proc:running() then
      error("the plaintext service exited before listening: " .. (proc:output() or ""), 0)
    end
    prova.sleep(50)
  end
  local front = tlspki.terminate(ctx, ctx, { pki = pki, upstream_port = plain_port })
  return { pki = pki, url = front.url }
end)

prova.test("ca_cert still rejects a good chain presented for the wrong hostname", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "the name is not the chain: a certificate this CA really signed is still the wrong certificate for this host, and a `ca_cert` that accepted it would have turned trust-this-CA into trust-anything-this-CA-ever-signed — including for a host the author never meant to reach",
  requires = GATES,
}, function(t)
  local m = t:use(mismatch)
  local out = eval(string.format([==[
local ok, e = pcall(function() return http.get(%q, { ca_cert = %q }) end)
return tostring(ok) .. " " .. tostring(e)
]==], m.url .. "/health", m.pki.ca))

  t:expect(out.stdout:find("false", 1, true) ~= nil,
    "the request fails: " .. out.stdout):equals(true)
  t:expect(out.stdout:find("wrong.example", 1, true) ~= nil,
    "…naming the certificate's actual name, so the diagnosis is one read: " .. out.stdout):equals(true)
end)

prova.test("insecure tolerates a hostname mismatch, and does so on every transport", {
  covers = "docs/design/architecture.md#tls-everywhere",
  proves = "reqwest checks the hostname behind a knob SEPARATE from the chain, while prova's own verifier ignores the name entirely — so `insecure` could have meant two different things on two transports, which is exactly the policy drift one shared parser is supposed to make impossible. Asserting it on http here and on wss/grpc in transports_test is what makes 'meaning identically' a measurement",
  requires = GATES,
}, function(t)
  local m = t:use(mismatch)
  local out = eval(string.format([==[
return http.get(%q, { insecure = true }).status
]==], m.url .. "/health"))

  t:expect(out.code, "the subject ran: " .. out.stdout):equals(0)
  t:expect(out.stdout:match("%d+"),
    "insecure ignores the name as well as the chain"):equals("200")
end)
