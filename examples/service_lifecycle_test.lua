--- POC example: the boot-then-probe acceptance loop with a *managed* process.
--- Run from the repo root: `prova examples/service_lifecycle_test.lua`
---
--- render/prepare → start the app (shell.spawn) → wait for it (http.wait_for) → probe its network
--- interface (http.get) → stop it on teardown (ctx:defer + proc:stop, run during async teardown).
--- This is the shape of a real service acceptance test; swap the python server for your built
--- binary (`shell.spawn("./target/release/app")`) and the rest is unchanged.

local service = prova.fixture("service", "file", function(ctx)
  -- Stand-in "service": a static server over a temp dir. In a real test this is your app.
  local root = ctx:tempdir()
  fs.write(root .. "/health", "ok")
  fs.write(root .. "/index.json", '{"status":"ok","name":"orders"}')

  local port = 8988
  local proc = ctx:manage(shell.spawn("python3 -m http.server " .. port .. " --directory " .. root))

  local base = "http://127.0.0.1:" .. port
  http.wait_for(base .. "/health", { status = 200, timeout = "10s", every = "100ms" })
  return { base = base, proc = proc }
end)

prova.test("the process is up with a pid", function(t)
  local svc = t:use(service)
  t:expect(svc.proc:running()):is_true()
  t:expect(svc.proc.pid):gt(0)
end)

prova.test("health endpoint is green", function(t)
  local res = http.get(t:use(service).base .. "/health")
  t:expect(res.status):equals(200)
  t:expect(res.body):contains("ok")
end)

prova.test("serves the json document", function(t)
  local res = http.get(t:use(service).base .. "/index.json")
  t:expect(res.status):equals(200)
  t:expect(res:json().status):equals("ok")
  t:expect(res:json().name):equals("orders")
end)
