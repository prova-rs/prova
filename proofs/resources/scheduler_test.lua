-- Dogfoods locks + the concurrency scheduler: declare a house rule once and the scheduler
-- co-schedules the parallelizable set safely — inert at --jobs 1, enforced above it, and held
-- across every prova instance at this home (the cross-instance leg lives in
-- cross_instance_test.lua). Readers-writer semantics: prova.writes is an exclusive hold,
-- prova.reads a concurrent one, and { serial } is run-wide. A bare token or prova.port is a
-- writer by default.

-- Two services that both bind :8080 exclusively — the scheduler will never overlap them.
prova.test("service A boots on :8080", { locks = { prova.port(8080) } }, function(t)
  prova.sleep(20)
  t:expect(true):is_true()
end)

prova.test("service B boots on :8080", { locks = { prova.port(8080) } }, function(t)
  prova.sleep(20)
  t:expect(true):is_true()
end)

-- Read-only tests against a shared database: these may run at the same time as each other…
prova.test("report reads the db", { locks = { prova.reads("db") } }, function(t)
  t:expect(1 + 1):equals(2)
end)

prova.test("dashboard reads the db", { locks = { prova.reads("db") } }, function(t)
  t:expect("ok"):equals("ok")
end)

-- …but a writer against the same db excludes all of them (writer waits for readers, blocks new).
prova.test("migration writes the db", { locks = { prova.writes("db") } }, function(t)
  t:expect(true):is_true()
end)

-- A destructive test that must own the whole run while it executes. Run-scoped by definition:
-- serial is this run's parallelism dial; a cross-instance rule is spelled as a lock.
prova.test("full reset (serial)", { serial = true }, function(t)
  t:expect(true):is_true()
end)

-- The pre-rename spelling still schedules and warns toward `locks` (retires at 1.0) — proven
-- in a SANDBOX child so this repo's own tree stays deprecation-clean: the bridge is behavior
-- worth a proof, not a warning worth printing on every load of our own suite.
prova.test("deprecated `resources` still schedules, warning toward `locks`", function(t)
  local pkg = t:tempdir()
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/bridge_test.lua", [[
prova.test("holds via the old spelling", { resources = { prova.reads("db") } }, function(t)
  t:expect(true):is_true()
end)
]])
  local r = shell.run({ prova.bin }, { cwd = pkg, merge_stderr = true, timeout = "60s" })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the bridge warns once, naming its successor"):contains("the option is `locks`")
end)
