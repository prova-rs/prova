--- What crosses the wire, in both directions, arrives intact and observable.
---
--- Three findings from one field session, all on the same surface:
---
---   * A binary response body was silently CORRUPTED
---     (docs/design/agent-ergonomics.md#http-binary-response-corrupted). `reqwest`'s `text()` is a
---     lossy UTF-8 conversion, so every invalid byte became U+FFFD — three bytes out for one byte
---     in — and a 22181-byte zip came back as 34220 unusable ones. What makes it worth a proof
---     rather than a note is how well it hid: status 200, a plausible `#body`, and `body:sub(1, 2)`
---     still "PK", because ASCII passes through untouched. A proof that sniffed the magic number
---     asserted nothing and reported green.
---   * `http` could not send a form body
---     (docs/design/agent-ergonomics.md#http-form-and-raw-bodies), so the two proofs that obtain an
---     OAuth token shelled out to `curl` — a host-tool `requires` on a proof whose subject is HTTP.
---   * `http` always followed redirects
---     (docs/design/agent-ergonomics.md#http-redirect-control), so "an unauthenticated visitor is
---     redirected to /auth/login" was unprovable: the client had already followed the 307 and
---     returned whatever the destination said.
---
--- The server is scripted rather than `python -m http.server` for the reason `probe_test.lua`
--- documents at length: stock `http.server` does a reverse-DNS lookup between bind() and listen(),
--- which costs ~70s on GitHub's macOS runners and makes readiness a coin flip.
local SERVER_PY = [[
import sys, io, os, json, zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from socketserver import TCPServer

# A REAL zip of high-entropy bytes: mostly invalid UTF-8, so lossy conversion cannot survive it,
# and a genuine archive, so the far end can be asked to open it rather than just measure it.
buf = io.BytesIO()
with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("payload.bin", os.urandom(8192))
ZIP = buf.getvalue()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
        elif self.path == "/artifact.zip":
            self.send_response(200)
            self.send_header("Content-Type", "application/zip")
            self.send_header("Content-Length", str(len(ZIP)))
            self.end_headers()
            self.wfile.write(ZIP)
        # A health endpoint behind auth: 401 without the bearer, 200 with it. The point is that a
        # poll which sends no header can NEVER reach the expected status, so a wait_for that
        # cannot authenticate fails by timing out rather than by asserting anything.
        elif self.path == "/private/health":
            if self.headers.get("Authorization") == "Bearer s3cr3t":
                self.send_response(200); self.end_headers(); self.wfile.write(b"ready")
            else:
                self.send_response(401); self.end_headers()
        elif self.path == "/gated":
            self.send_response(307)
            self.send_header("Location", "/auth/login")
            self.end_headers()
        elif self.path == "/auth/login":
            self.send_response(200); self.end_headers(); self.wfile.write(b"login page")
        else:
            self.send_response(404); self.end_headers()
    # Every POST echoes what it received, so an assertion is about the bytes SENT, not about a
    # server's interpretation of them.
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({
            "content_type": self.headers.get("Content-Type", ""),
            "body": body.decode("utf-8", "replace"),
        }).encode())

class S(ThreadingHTTPServer):
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name, self.server_port = "localhost", self.server_address[1]

S(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
]]

-- Windows resolves `python3` to the Store app-execution alias — a stub that runs forever without
-- ever binding. The real interpreter there is `python`; everywhere else `python3` is reliable.
local python = package.config:sub(1, 1) == "\\" and "python" or "python3"

local service = prova.fixture("payload-service", Scope.File, function(ctx)
  local root = ctx:tempdir()
  fs.write(root .. "/serve.py", SERVER_PY)
  local port = net.free_port()
  ctx:manage(shell.spawn({ python, root .. "/serve.py", tostring(port) }, { cwd = root }))
  local base = "http://127.0.0.1:" .. port
  http.wait_for(base .. "/health", { status = 200, timeout = "30s", every = "100ms" })
  return { url = base, dir = root }
end)

--- Every `http` call under proof runs in the SUBJECT, never here.
---
--- This is the trap CLAUDE.md names and this repo has been bitten by: `http.get(…)` written
--- directly in a proof body exercises whichever prova is CONDUCTING the suite — usually an
--- installed one, built from who-knows-what — and says nothing about the tree under test. The
--- symptom is a proof that goes green the moment you have a good binary on PATH, including on the
--- commit that broke the feature. The server above is infrastructure, so it stays here; the
--- feature reaches through `prova.bin`.
---
--- The snippet returns one line, which `eval` prints, so an assertion can be made on the far
--- side of the process boundary.
local function eval(code)
  local r = shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "60s" })
  return r
end

prova.test("a binary body survives the round trip byte for byte", {
  covers = "docs/design/agent-ergonomics.md#http-binary-response-corrupted",
  proves = "the corruption was invisible to every cheap check — status, a plausible length, and a magic number all passed — so the only assertion that can catch it is one that compares the WHOLE payload against an independent reading of the same bytes",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  local r = eval(string.format([==[
local res = http.get(%q)
return res.status .. " " .. #res.body .. " " .. res.headers["content-length"] .. " " .. res.body:sub(1, 2)
]==], svc.url .. "/artifact.zip"))

  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  local status, len, declared, magic = r.stdout:match("(%d+) (%d+) (%d+) (%S+)")
  t:expect(status, "the fetch succeeds"):equals("200")
  -- The declared length is the SERVER's own count, so this compares the subject's body against a
  -- number prova had no part in producing. Under the defect it read ~1.5x this.
  t:expect(len, "the body is exactly as long as the server said it was"):equals(declared)
  t:expect(magic, "…and still starts with the zip magic"):equals("PK")
end)

prova.test("`save` writes an archive a foreign tool opens without complaint", {
  covers = "docs/design/agent-ergonomics.md#http-binary-response-corrupted",
  proves = "byte-equal length is necessary and not sufficient: the payload has to be USABLE, and the only witness that cannot be fooled by a plausible-looking body is a reader that was never told what to expect — python's zipfile called the corrupted download a 'corrupt member' while prova called it 200 OK",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  local out = svc.dir .. "/downloaded.zip"
  local r = eval(string.format([==[
return http.get(%q):save(%q)
]==], svc.url .. "/artifact.zip", out))

  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  t:expect(r.stdout:gsub("%s+$", ""), "save returns the path, so it chains"):equals(out)
  -- `testzip()` returns the name of the first corrupt member, or None when every CRC checks out.
  -- Printing it makes a failure name the member rather than merely asserting badness.
  local verdict = shell.run({ python, "-c", [[
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
bad = z.testzip()
print("CORRUPT:" + bad if bad else "CLEAN:" + str(len(z.read("payload.bin"))))
]], out })
  t:expect(verdict.code, "the reader ran: " .. verdict.stderr):equals(0)
  t:expect(verdict.stdout, "every member's CRC checks out"):contains("CLEAN:")
  t:expect(verdict.stdout, "…and the member is its original size"):contains("8192")
end)

prova.test("a form body goes out form-encoded, without shelling out to curl", {
  covers = "docs/design/agent-ergonomics.md#http-form-and-raw-bodies",
  proves = "OAuth 2.0 token endpoints require application/x-www-form-urlencoded, and its absence put a host-tool `requires` on proofs whose subject is HTTP — the encoding has to be asserted on the wire, since a body that merely LOOKS right fails at the far end of an exchange whose error names none of this",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  local r = eval(string.format([==[
local e = http.post(%q, { form = { grant_type = "password", username = "a b&c" } }):json()
return e.content_type .. " || " .. e.body
]==], svc.url .. "/echo"))

  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  t:expect(r.stdout, "the content type is the form's, unasked"):contains("application/x-www-form-urlencoded")
  t:expect(r.stdout, "a field is encoded as a pair"):contains("grant_type=password")
  -- The characters that break a hand-rolled encoder: a space becomes `+` and the `&` that would
  -- otherwise split one field into two is escaped.
  t:expect(r.stdout, "a space and an ampersand survive escaping"):contains("username=a+b%26c")
end)

prova.test("a raw body is sent verbatim, under the content type named", {
  covers = "docs/design/agent-ergonomics.md#http-form-and-raw-bodies",
  proves = "`body` + `content_type` is the escape hatch for every media type prova will never enumerate; without it the only way to send XML or a signed blob was a host tool",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  local r = eval(string.format([==[
local e = http.post(%q, { body = "<note><to>prova</to></note>", content_type = "application/xml" }):json()
return e.content_type .. " || " .. e.body
]==], svc.url .. "/echo"))

  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  t:expect(r.stdout, "the declared type is what was sent"):contains("application/xml")
  t:expect(r.stdout, "the bytes are untouched"):contains("<note><to>prova</to></note>")
end)

prova.test("a redirect can be observed instead of followed", {
  covers = "docs/design/agent-ergonomics.md#http-redirect-control",
  proves = "auth flows ARE redirects — login, callback, logout — so a suite that cannot see the hop cannot prove the gate; the default had already followed the 307 and returned the destination's answer, which in the field was a 500 from a deliberately-absent identity provider and read as a broken app",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  -- Both halves in one snippet: the option AND the default, from the same binary, so the pair
  -- cannot drift apart across two subject invocations.
  local r = eval(string.format([==[
local seen = http.get(%q, { redirects = false })
local followed = http.get(%q)
return seen.status .. " " .. seen.headers.location .. " || " .. followed.status .. " " .. followed.body
]==], svc.url .. "/gated", svc.url .. "/gated"))

  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  local held, dest, code, body = r.stdout:match("(%d+) (%S+) || (%d+) (.-)%s*$")
  t:expect(held, "the 3xx itself is returned"):equals("307")
  t:expect(dest, "…with the destination intact, which is the assertion worth making"):equals("/auth/login")

  -- The negative control: without the option the hop is invisible, which is both the old behavior
  -- and the right default. A proof of the new option that did not pin the default would not
  -- notice if `redirects = false` had quietly become the only behavior.
  t:expect(code, "the default still follows"):equals("200")
  t:expect(body, "…and lands on the destination"):equals("login page")
end)

prova.test("a readiness poll can authenticate, on the free verb and the client alike", {
  covers = "docs/design/agent-ergonomics.md#http-wait-for-cannot-authenticate",
  proves = "readiness against a service behind auth was unprovable: the free verb sent no headers, so a health endpoint requiring a bearer could only ever answer 401 and the wait timed out — which reads as 'the service never came up', the most misleading possible diagnosis. `client:wait_for` carried the client's defaults all along, so the two verbs disagreed about whether polling could authenticate at all",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  -- NOTE: the snippet must not START with a Lua comment — `prova eval` receives it as one argv
  -- element, and an argument beginning with `--` is parsed as a flag (exit 2). See
  -- agent-ergonomics.md#eval-snippet-starting-with-a-comment.
  local r = eval(string.format([==[
local direct = http.wait_for(%q, {   -- the free verb, with a per-call header
  status = 200, headers = { Authorization = "Bearer s3cr3t" }, timeout = "10s", every = "100ms",
})
-- The client, whose default header must reach the poll without being restated.
local c = http.client{ base_url = %q, headers = { Authorization = "Bearer s3cr3t" } }
local viaclient = c:wait_for("/private/health", { status = 200, timeout = "10s", every = "100ms" })
-- And a per-call header must OVERRIDE a client default by name, the same precedence an ordinary
-- request gets — otherwise the two verbs disagree about whose Authorization wins.
local wrong = http.client{ base_url = %q, headers = { Authorization = "Bearer nope" } }
local overridden = wrong:wait_for("/private/health", {
  status = 200, headers = { Authorization = "Bearer s3cr3t" }, timeout = "10s", every = "100ms",
})
return direct.status .. " " .. direct.body .. " | " .. viaclient.status .. " | " .. overridden.status
]==], svc.url .. "/private/health", svc.url, svc.url))

  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  local status, body, client_status, override_status =
    r.stdout:match("(%d+) (%S+) | (%d+) | (%d+)")
  t:expect(status, "the free verb reached the guarded endpoint"):equals("200")
  t:expect(body, "…and it is really the guarded body, not a redirect to one"):equals("ready")
  t:expect(client_status, "the client's default header reaches its poll"):equals("200")
  t:expect(override_status, "a per-call header overrides the client's by name"):equals("200")
end)

prova.test("a poll that cannot authenticate fails as a timeout, which is why this needed fixing", {
  covers = "docs/design/agent-ergonomics.md#http-wait-for-cannot-authenticate",
  proves = "the negative control, and the reason the gap was expensive rather than merely missing: without headers the poll never reaches the expected status, so the failure arrives as 'did not come up in 2s' — a diagnosis pointing at the service instead of at the request",
  requires = { "python3" },
}, function(t)
  local svc = t:use(service)
  local r = eval(string.format([==[
local ok, err = pcall(function()
  return http.wait_for(%q, { status = 200, timeout = "2s", every = "100ms" })
end)
return tostring(ok) .. " | " .. tostring(err)
]==], svc.url .. "/private/health"))

  t:expect(r.code, "the subject ran"):equals(0)
  t:expect(r.stdout, "an unauthenticated poll never reaches 200"):contains("false")
  t:expect(r.stdout, "…and says so as a timeout, naming the status it wanted"):contains("timed out")
end)
