-- Dogfoods the boot-then-probe pattern (distinct from http.mock): a file-scoped fixture starts a real
-- service as a MANAGED process (shell.spawn + ctx:manage → stopped on teardown), waits for health with
-- http.wait_for, and the tests probe it with the http module. Python's stdlib http.server stands in
-- for the service under test; gated on python3 so it skips cleanly where that is unavailable.
local service = prova.fixture("service", Scope.File, function(ctx)
  local root = ctx:tempdir()
  fs.write(root .. "/health", "ok")
  fs.write(root .. "/index.json", '{"status":"ok","name":"demo"}')

  local port = 8987
  -- shell.spawn returns a managed process handle; ctx:manage stops it during async teardown.
  local proc = ctx:manage(shell.spawn("python3 -m http.server " .. port .. " --directory " .. root))

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
