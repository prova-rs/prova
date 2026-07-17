-- PROOF (readiness) — `wait` is a CONTRACT, not a hint. When `docker.run` returns, the container is
-- READY: a client's FIRST probe succeeds. Anything weaker is a false-ready, and a false-ready is
-- worse than no wait at all, because it moves the failure somewhere confusing and makes suites pass
-- on luck (measured: it was an image pull's latency that kept an earlier proof green).
--
-- Why `wait = { port }` was a false-ready: it connected to the MAPPED HOST port, and Docker Desktop's
-- port proxy accepts that connection the moment the container starts — before anything inside is
-- listening. The signal said "ready" while the server was still booting.
--
-- The probe here is deliberately given NO margin: the prober container is started BEFORE the
-- database, so there is no container-start latency between "ready" and the probe to hide behind.
-- Every probe is a single attempt — no prova.retry. If `wait` is honest, one attempt is enough.
--
-- Run standalone: prova crates/prova-core/testdata/docker_readiness.lua   (requires docker)

prova.test("wait = { port } is a TRUE readiness signal: the first probe succeeds",
           { requires = { "docker" } }, function(t)
  local net = t:manage(docker.network())

  -- Started FIRST, so it is already running and warm when the database reports ready. Nothing pads
  -- the gap between wait returning and the probe landing.
  local prober = t:manage(docker.run{
    image = "postgres:16-alpine", network = net, command = "sleep 120",
  })

  local db = t:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_PASSWORD = "secret" },
    ports = { 5432 },
    network = net, alias = "db",
    wait = { port = 5432, timeout = "60s" },
  })

  -- docker.run has returned. The contract says READY, not "started". ONE attempt, no retry.
  local out = prober:run({
    "env", "PGPASSWORD=secret", "psql", "-h", "db", "-U", "postgres", "-tAc", "select 42",
  })
  t:expect(out, "the first probe after wait={port} returned"):contains("42")
end)

prova.test("readiness holds for an UNPUBLISHED port — in-network resources are waitable too",
           { requires = { "docker" } }, function(t)
  -- A resource reachable only on the network (no host publish) is a legitimate topology member: a
  -- containerized SUT talks to it by alias, and the host never does. A readiness check that can only
  -- see mapped host ports cannot wait for one at all — so this case proves the check is looking at
  -- the container, not at the host's port map.
  local net = t:manage(docker.network())
  local prober = t:manage(docker.run{
    image = "postgres:16-alpine", network = net, command = "sleep 120",
  })

  local db = t:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_PASSWORD = "secret" },
    -- NO `ports` — nothing published to the host.
    network = net, alias = "hidden",
    wait = { port = 5432, timeout = "60s" },
  })

  -- Nothing is published: there is no host mapping to probe, so a host-side check could not even
  -- ask the question. The wait above still had to work.
  local published = pcall(function() return db:host_port(5432) end)
  t:expect(published, "an unpublished port has no host mapping"):equals(false)

  local out = prober:run({
    "env", "PGPASSWORD=secret", "psql", "-h", "hidden", "-U", "postgres", "-tAc", "select 42",
  })
  t:expect(out):contains("42")
end)

prova.test("a container that never listens fails the wait, and says so",
           { requires = { "docker" } }, function(t)
  -- The other half of the contract: readiness must be able to say NO. A container that starts fine
  -- but never listens on the port must time out rather than be waved through — otherwise "ready"
  -- means nothing.
  local ok, err = pcall(function()
    return t:manage(docker.run{
      image = "postgres:16-alpine",
      command = "sleep 120",                       -- starts, but nothing ever listens on 5432
      ports = { 5432 },
      wait = { port = 5432, timeout = "5s", every = "250ms" },
    })
  end)
  t:expect(ok, "a container that never listens must not report ready"):equals(false)
  t:expect(tostring(err)):contains("not ready")
end)
