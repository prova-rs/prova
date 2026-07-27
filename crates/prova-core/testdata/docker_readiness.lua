-- PROOF (readiness) — `wait` is a CONTRACT, not a hint. When `docker.run` returns, the container is
-- READY: a client's FIRST probe succeeds. Anything weaker is a false-ready, and a false-ready is
-- worse than no wait at all, because it moves the failure somewhere confusing and makes suites pass
-- on luck (measured: it was an image pull's latency that kept an earlier proof green).
--
-- Readiness has no single universal signal, so `wait` offers three, each HONEST about a different
-- observable — and the author picks the one that is true for their service:
--
--   * `wait = { port }`  — the port is in LISTEN state INSIDE the container (`/proc/net/tcp`, not the
--     mapped host port: Docker Desktop's port proxy accepts the moment the container starts, so a
--     host-side check passes while the server is still booting and never fails for a container that
--     never listens). Honest about the PORT. Use it when listening == serving (Redis, nginx).
--   * `wait = { cmd }`   — a readiness COMMAND run in the container; ready ⇔ it exits 0. The general
--     signal for a server whose socket predates its serving: Postgres binds TCP and THEN finishes
--     startup, rejecting queries with "the database system is starting up" in the gap — so a `port`
--     probe races, and `pg_isready` (non-zero during that gap) does not.
--   * `wait = { log }`   — a line the server prints when ready. (Covered by docker.lua.)
--
-- The probe is deliberately given NO margin: the prober container is started BEFORE the server, so
-- there is no container-start latency between "ready" and the probe to hide behind. Every probe is a
-- single attempt — no prova.retry. If `wait` is honest, one attempt is enough.
--
-- Run standalone: prova crates/prova-core/testdata/docker_readiness.lua   (requires docker)

prova.test("wait = { port } is a TRUE readiness signal: the first probe succeeds",
           { requires = { "docker" } }, function(t)
  local net = t:manage(docker.network())

  -- Started FIRST, so it is already running and warm when the server reports ready. Nothing pads
  -- the gap between wait returning and the probe landing.
  local prober = t:manage(docker.run{
    image = "redis:7-alpine", network = net, command = "sleep 120",
  })

  -- Redis is the honest server for a PORT probe: its listening socket and its ability to serve
  -- appear together, so LISTEN state IS readiness — no gap for the probe to fall into (contrast the
  -- `cmd` test below). `--protected-mode no` lets a sibling reach it over the network unauthenticated.
  t:manage(docker.run{
    image = "redis:7-alpine",
    command = "redis-server --protected-mode no",
    ports = { 6379 },
    network = net, alias = "cache",
    wait = { port = 6379, timeout = "60s" },
  })

  -- docker.run has returned. The contract says READY, not "started". ONE attempt, no retry.
  local out = prober:run({ "redis-cli", "-h", "cache", "ping" })
  t:expect(out, "the first probe after wait={port} returned"):contains("PONG")
end)

prova.test("wait = { cmd } gates on a real readiness check — the honest signal when a port would race",
           { requires = { "docker" } }, function(t)
  -- The reason `cmd` exists. Postgres binds its TCP socket and THEN finishes startup: for a window
  -- the port is in LISTEN state but every query is rejected with "the database system is starting
  -- up". `wait = { port }` would report ready inside that window and the first query would fail on
  -- luck. `wait = { cmd = { pg_isready ... } }` runs Postgres's own readiness check — non-zero
  -- during the gap, zero only once it is genuinely serving — so the FIRST query lands on a live
  -- database. `-h 127.0.0.1` pins the check to the TCP path the client uses (the temporary
  -- init-phase server listens only on a unix socket, so it cannot spoof this into a false-ready).
  local net = t:manage(docker.network())
  local prober = t:manage(docker.run{
    image = "postgres:16-alpine", network = net, command = "sleep 120",
  })

  t:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_PASSWORD = "secret" },
    ports = { 5432 },
    network = net, alias = "db",
    wait = { cmd = { "pg_isready", "-h", "127.0.0.1", "-U", "postgres" }, timeout = "60s" },
  })

  -- ONE attempt, no retry: cmd readiness means the query lands on a serving database.
  local out = prober:run({
    "env", "PGPASSWORD=secret", "psql", "-h", "db", "-U", "postgres", "-tAc", "select 42",
  })
  t:expect(out, "the first query after wait={cmd} returned"):contains("42")
end)

prova.test("readiness holds for an UNPUBLISHED port — in-network resources are waitable too",
           { requires = { "docker" } }, function(t)
  -- A resource reachable only on the network (no host publish) is a legitimate topology member: a
  -- containerized SUT talks to it by alias, and the host never does. A readiness check that can only
  -- see mapped host ports cannot wait for one at all — so this case proves the check is looking at
  -- the container, not at the host's port map. Redis again: the PORT probe is what is under test, so
  -- the server must be one where LISTEN == serving.
  local net = t:manage(docker.network())
  local prober = t:manage(docker.run{
    image = "redis:7-alpine", network = net, command = "sleep 120",
  })

  local db = t:manage(docker.run{
    image = "redis:7-alpine",
    command = "redis-server --protected-mode no",
    -- NO `ports` — nothing published to the host.
    network = net, alias = "hidden",
    wait = { port = 6379, timeout = "60s" },
  })

  -- Nothing is published: there is no host mapping to probe, so a host-side check could not even
  -- ask the question. The wait above still had to work.
  local published = pcall(function() return db:host_port(6379) end)
  t:expect(published, "an unpublished port has no host mapping"):equals(false)

  local out = prober:run({ "redis-cli", "-h", "hidden", "ping" })
  t:expect(out):contains("PONG")
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
