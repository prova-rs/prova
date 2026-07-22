-- Dogfoods the container-dependency pattern: stand a real container up as a scoped fixture, map a
-- random host port, wait for readiness, drive it over the network, and remove it on teardown — how a
-- proof stands up a real Postgres/Kafka/etc. under the system-under-test.
local service = prova.fixture("service", Scope.File, function(ctx)
  return ctx:manage(docker.run{
    image = "traefik/whoami",              -- tiny public HTTP echo on :80
    ports = { 80 },                        -- published to a random host port
    wait = { port = 80, timeout = "60s" }, -- ready when the port accepts connections
  })                                       -- ctx:manage → removed during async teardown
end)

prova.group("containerized dependency", { requires = { "docker" } }, function(g)
  g:test("responds over the mapped port", function(t)
    local c = t:use(service)
    local res = http.get("http://" .. c:endpoint(80) .. "/")
    t:expect(res.status):equals(200)
    t:expect(res.body):contains("Hostname")
  end)

  g:test("publishes a real host port", function(t)
    t:expect(t:use(service):host_port(80)):gt(1024)
  end)
end)
