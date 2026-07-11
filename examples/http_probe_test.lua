--- POC example: the `http` module — boot a service, wait for it, probe it.
--- Run from the repo root: `prova examples/http_probe_test.lua`
---
--- Uses Python's stdlib http.server as a stand-in "service under test" (ubiquitous on dev/CI). A
--- file-scoped fixture serves a temp dir in the background, waits for health with http.wait_for,
--- and tears the server down at file end. This is the boot-then-probe acceptance pattern.

local server = prova.fixture("server", "file", function(ctx)
  local root = ctx:tempdir()
  fs.write(root .. "/health", "ok")
  fs.write(root .. "/index.json", '{"status":"ok","name":"demo"}')

  -- Serve on a fixed port in the background; stop it when the file's tests finish.
  local port = 8987
  shell.run("python3 -m http.server " .. port .. " --directory " .. root .. " >/dev/null 2>&1 &")
  ctx:defer(function()
    shell.run("pkill -f 'http.server " .. port .. "'")
  end)

  local base = "http://127.0.0.1:" .. port
  http.wait_for(base .. "/health", { status = 200, timeout = "10s", every = "100ms" })
  return base
end)

prova.test("health endpoint is up", function(t)
  local res = http.get(t:use(server) .. "/health")
  t:expect(res.status):equals(200)
  t:expect(res.body):contains("ok")
end)

prova.test("serves the json document", function(t)
  local res = http.get(t:use(server) .. "/index.json")
  t:expect(res.status):equals(200)
  t:expect(res:json().status):equals("ok")
  t:expect(res:json().name):equals("demo")
end)

prova.test("unknown path is a 404", function(t)
  local res = http.get(t:use(server) .. "/nope")
  t:expect(res.status):equals(404)
end)
