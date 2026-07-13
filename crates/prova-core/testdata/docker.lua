-- A containerized dependency as a fixture: started on first use, stopped on (async) teardown.
-- Gated by `requires = { "docker" }` on the group, so where docker is absent these SKIP (and the
-- fixture, being lazy, never starts a container).
local whoami = prova.fixture("whoami", Scope.File, function(ctx)
  local c = docker.run{
    image = "traefik/whoami",       -- tiny public image; HTTP echo on :80
    ports = { 80 },                 -- publish to a random host port
    wait = { port = 80, timeout = "60s" },
  }
  ctx:defer(function() c:stop() end)
  return c
end)

prova.group("containerized service", { requires = { "docker" } }, function(g)
  g:test("serves http on the mapped host port", function(t)
    local c = t:use(whoami)
    local res = http.get("http://" .. c:endpoint(80) .. "/")
    t:expect(res.status):equals(200)
    t:expect(res.body):contains("Hostname")
  end)

  g:test("publishes a nonzero host port", function(t)
    t:expect(t:use(whoami):host_port(80)):gt(0)
  end)
end)

-- Exercises the bollard exec, logs, and log-based readiness paths on a shell-capable image.
prova.group("redis exec and logs", { requires = { "docker" } }, function(g)
  g:test("exec runs, logs stream, wait.log gates readiness", function(t)
    local c = docker.run{
      image = "redis:alpine",
      ports = { 6379 },
      wait = { log = "Ready to accept connections", timeout = "30s" },  -- polls container_logs
    }
    local code, out = c:exec("echo hi-from-exec")   -- redis:alpine has a shell
    t:expect(code):equals(0)
    t:expect(out):contains("hi-from-exec")
    t:expect(c:logs()):contains("Ready to accept connections")
    c:stop()
  end)
end)
