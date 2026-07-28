-- Dogfoods the boot-then-probe pattern (distinct from http.mock): a file-scoped fixture starts a real
-- service as a MANAGED process (shell.spawn + ctx:manage → stopped on teardown), waits for health with
-- http.wait_for, and the tests probe it with the http module. A tiny stdlib-Python HTTP server stands
-- in for the service under test; gated on python3 so it skips cleanly where that is unavailable.
--
-- Why a scripted server and not `python -m http.server`: stock http.server's server_bind() calls
-- socket.getfqdn(host) — a reverse-DNS lookup — BETWEEN bind() and listen(). On a host with slow
-- reverse DNS (measured: ~70s on GitHub's macOS runners) the socket sits bound-but-not-listening for
-- that whole time, so the port is unreachable and any readiness wait times out. A test service must
-- boot deterministically, not gate on the host's resolver, so we bind to a literal IP and skip getfqdn.
local SERVER_PY = [[
import sys, os
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from socketserver import TCPServer
port, root = int(sys.argv[1]), sys.argv[2]
os.chdir(root)
class Server(ThreadingHTTPServer):
    def server_bind(self):           # skip HTTPServer.server_bind's socket.getfqdn() reverse lookup
        TCPServer.server_bind(self)
        self.server_name, self.server_port = "localhost", self.server_address[1]
Server(("127.0.0.1", port), SimpleHTTPRequestHandler).serve_forever()
]]

local service = prova.fixture("service", Scope.File, function(ctx)
  local root = ctx:tempdir()
  fs.write(root .. "/health", "ok")
  fs.write(root .. "/index.json", '{"status":"ok","name":"demo"}')
  fs.write(root .. "/serve.py", SERVER_PY)

  local port = 8987
  -- shell.spawn returns a managed process handle; ctx:manage stops it during async teardown.
  local proc = ctx:manage(shell.spawn("python3 " .. root .. "/serve.py " .. port .. " " .. root))

  local base = "http://127.0.0.1:" .. port
  http.wait_for(base .. "/health", { status = 200, timeout = "10s", every = "100ms" })
  return { base = base, proc = proc }
end)

prova.group("boot-then-probe a managed service", { requires = { "python3" } }, function(g)
  g:test("the process is up with a pid", function(t)
    local svc = t:use(service)
    t:expect(svc.proc:running()):is_true()
    t:expect(svc.proc.pid):gt(0)
  end)

  g:test("health endpoint is up", function(t)
    local res = http.get(t:use(service).base .. "/health")
    t:expect(res.status):equals(200)
    t:expect(res.body):contains("ok")
  end)

  g:test("serves the json document", function(t)
    local res = http.get(t:use(service).base .. "/index.json")
    t:expect(res.status):equals(200)
    t:expect(res:json().status):equals("ok")
    t:expect(res:json().name):equals("demo")
  end)

  g:test("unknown path is a 404", function(t)
    local res = http.get(t:use(service).base .. "/nope")
    t:expect(res.status):equals(404)
  end)
end)
