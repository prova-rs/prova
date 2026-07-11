--- POC example: resources & the concurrency scheduler.
---
--- Resources declare *external* constraints so the scheduler co-schedules the parallelizable set
--- safely. They are inert at `--jobs 1` and enforced above it — declare once, scale out without
--- touching tests. Run with, e.g., `prova examples/resources_test.lua --jobs 8`.
---
--- Semantics (readers-writer):
---   • exclusive (default): `prova.port(n)`, `prova.resource(tok)`, or a bare string — writer.
---   • shared: `prova.shared(x)` — concurrent reader; readers overlap, a writer waits for them.
---   • serial: `{ serial = true }` — process-wide exclusive; never concurrent with anything.

-- Two services that both bind :8080 exclusively — the scheduler will never overlap them.
prova.test("service A boots on :8080", { resources = { prova.port(8080) } }, function(t)
  prova.sleep(20)
  t:expect(true):is_true()
end)

prova.test("service B boots on :8080", { resources = { prova.port(8080) } }, function(t)
  prova.sleep(20)
  t:expect(true):is_true()
end)

-- Read-only tests against a shared database: these may run at the same time as each other…
prova.test("report reads the db", { resources = { prova.shared("db") } }, function(t)
  t:expect(1 + 1):equals(2)
end)

prova.test("dashboard reads the db", { resources = { prova.shared("db") } }, function(t)
  t:expect("ok"):equals("ok")
end)

-- …but a writer against the same db excludes all of them (writer waits for readers, blocks new).
prova.test("migration writes the db", { resources = { prova.resource("db") } }, function(t)
  t:expect(true):is_true()
end)

-- A destructive test that must own the whole world while it runs.
prova.test("full reset (serial)", { serial = true }, function(t)
  t:expect(true):is_true()
end)
