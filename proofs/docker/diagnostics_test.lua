-- PROOF (docker diagnostics) — `docker.diagnostics()` is the contract a soak measures a container
-- runtime through, so it has to mean something.
--
-- prova silently survives a runtime that exposes a port and binds nothing to it: it replaces the
-- container and hands the caller a working one. Silence is right for the caller and wrong for
-- measurement — "2000 starts, all fine" and "2000 starts, 3 of which this runtime botched and we
-- healed" are completely different findings, and without counters they are the same observation.
--
-- The recovery MACHINERY is proven in Rust (`modules::docker::tests`), where the fault can be
-- injected directly and the rare defect made to happen on demand. What can only be checked from
-- out here is the part a soak actually leans on: that the numbers are reachable from Lua, and that
-- they measure the RUNTIME rather than prova's own bookkeeping. A counter that drifted on healthy
-- starts would make every soak number noise.
--
-- Run standalone: prova proofs/docker/diagnostics_test.lua   (requires docker)

prova.test("diagnostics are readable and start as plain numbers", function(t)
  local d = docker.diagnostics()
  t:expect(type(d.port_bind_recoveries), "recoveries counter is a number"):equals("number")
  t:expect(type(d.port_bind_failures), "failures counter is a number"):equals("number")
  -- Monotonic counters, so a reader takes a delta rather than an absolute.
  t:expect(d.port_bind_recoveries, "counters are non-negative"):gte(0)
  t:expect(d.port_bind_failures, "counters are non-negative"):gte(0)
end)

prova.test("a healthy container start moves neither counter", { requires = { "docker" } }, function(t)
  local before = docker.diagnostics()

  local c = t:manage(docker.run{
    image = "alpine:3.20",
    command = { "sleep", "10" },
    ports = { 80 },
  })
  t:expect(c:host_port(80), "an ordinary container publishes a host port"):gt(0)

  local after = docker.diagnostics()
  -- This is the assertion that makes a soak legible: on a well-behaved runtime the numbers must
  -- stay put, so any movement during a soak is the runtime misbehaving, never prova being busy.
  t:expect(after.port_bind_recoveries - before.port_bind_recoveries,
           "no recovery recorded for a start that never needed one"):equals(0)
  t:expect(after.port_bind_failures - before.port_bind_failures,
           "no failure recorded for a start that succeeded"):equals(0)
end)
