-- Dogfoods resources + the concurrency scheduler: declare external constraints once and the
-- scheduler co-schedules the parallelizable set safely — inert at --jobs 1, enforced above it.
-- Readers-writer semantics: prova.writes is an exclusive hold, prova.reads a concurrent one, and
-- { serial } is process-wide. A bare token or prova.port is a writer by default.

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
prova.test("report reads the db", { resources = { prova.reads("db") } }, function(t)
  t:expect(1 + 1):equals(2)
end)

prova.test("dashboard reads the db", { resources = { prova.reads("db") } }, function(t)
  t:expect("ok"):equals("ok")
end)

-- …but a writer against the same db excludes all of them (writer waits for readers, blocks new).
prova.test("migration writes the db", { resources = { prova.writes("db") } }, function(t)
  t:expect(true):is_true()
end)

-- A destructive test that must own the whole world while it runs.
prova.test("full reset (serial)", { serial = true }, function(t)
  t:expect(true):is_true()
end)
