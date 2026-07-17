-- PROOF 1 (networked topologies) — written before the implementation. The foundation the
-- SUT-in-a-container work stands on: a user-defined Docker network, containers joined to it with a
-- stable alias, and — the real proof, not just field presence — a SIBLING container reaching
-- another by that alias over the network's embedded DNS. A container stays dual-homed: its existing
-- host vantage (host_port(p), the mapped port) works AND it answers to its alias on the network.
--
-- Run standalone: prova crates/prova-core/testdata/docker_network.lua   (requires docker)

prova.test("a sibling container reaches a resource by network alias", { requires = { "docker" } }, function(t)
  -- A managed user-defined network — removed at scope end, after its containers (LIFO).
  local net = t:manage(docker.network())
  t:expect(net.name, "a named user-defined network"):matches("prova")

  -- The "resource": dual-homed — published to a host port AND joined to the network as `db`.
  local db = t:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_PASSWORD = "secret" },
    ports = { 5432 },                     -- host vantage (the test runner's mapped port)
    network = net, alias = "db",          -- network vantage (siblings resolve `db` by DNS)
    wait = { port = 5432, timeout = "60s" },
  })

  -- Host vantage unchanged: the mapped (random) host port, not the container port.
  t:expect(db:host_port(5432)):never():equals(5432)
  -- Network vantage: the alias this container answers to on the network.
  t:expect(db:network_alias()):equals("db")

  -- THE PROOF: a second container on the same network resolves `db` by DNS and connects to it.
  -- (postgres:16-alpine ships psql, so the client image is the same.)
  local client = t:manage(docker.run{
    image = "postgres:16-alpine",
    network = net,
    command = "sleep 120",                -- keep it alive so we can exec into it
  })

  -- No retry: `wait = { port = 5432 }` above is a TRUE readiness contract (it asks the container's
  -- own kernel what is listening — see testdata/docker_readiness.lua), so the first probe succeeds.
  -- This line briefly carried a prova.retry to paper over a false-ready; fixing the signal made the
  -- workaround removable, which is the honest end state.
  local out = client:run({
    "env", "PGPASSWORD=secret",
    "psql", "-h", "db", "-U", "postgres", "-tAc", "select 42",
  })
  t:expect(out):contains("42")
end)
