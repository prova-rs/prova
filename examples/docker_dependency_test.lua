--- POC example: an ephemeral containerized dependency (testcontainers-style).
--- Run from the repo root: `prova examples/docker_dependency_test.lua`
--- Requires docker; where it is unavailable these tests SKIP (never fail) via `requires`.
---
--- Spin up a real container as a scoped fixture → map a random host port → wait for readiness →
--- drive it over its network interface → remove it on (async) teardown. This is how a test stands
--- up a real Postgres/Kafka/etc. under the system-under-test.

local service = prova.fixture("service", "file", function(ctx)
  local c = docker.run{
    image = "traefik/whoami",              -- tiny public HTTP echo on :80
    ports = { 80 },                        -- published to a random host port
    wait = { port = 80, timeout = "60s" }, -- ready when the port accepts connections
  }
  ctx:defer(function() c:stop() end)       -- removed during async teardown
  return c
end)

-- `requires` gates the whole group: no docker → these skip, and the fixture never starts.
prova.group("containerized whoami", { requires = { "docker" } }, function(g)
  g:test("responds 200 over the mapped port", function(t)
    local c = t:use(service)
    local res = http.get("http://" .. c:endpoint(80) .. "/")
    t:expect(res.status):equals(200)
    t:expect(res.body):contains("Hostname")
  end)

  g:test("publishes a real host port", function(t)
    t:expect(t:use(service):host_port(80)):gt(1024)
  end)
end)
-- (`:exec` runs `sh -c` in the container, so it needs a shell in the image — traefik/whoami is
--  FROM scratch and has none. Use an image like alpine/busybox to demo `:exec`.)
