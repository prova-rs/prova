-- A containerized dependency as a fixture: started on first use, stopped on (async) teardown.
-- Gated by `requires = { "docker" }` on the group, so where docker is absent these SKIP (and the
-- fixture, being lazy, never starts a container).
local whoami = prova.fixture("whoami", "file", function(ctx)
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
