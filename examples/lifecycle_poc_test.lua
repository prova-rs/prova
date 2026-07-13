--- POC example: fixtures, scoping, and lifecycle only — no archetect, no network.
--- This is the target the first Rust prototype should be able to run end to end.
---
--- Proves: the three scopes cache correctly (suite built once, file once, test every time),
--- fixture-to-fixture dependencies resolve, and teardown runs LIFO / inner-scope-first.

-- `prova` and the fs/shell/http modules are injected globals — no require needed.

-- suite-scoped: constructed once for the whole run, torn down last.
local suite_dir = prova.fixture("suite_dir", Scope.Suite, function(ctx)
  local dir = ctx:tempdir()
  ctx:log("suite_dir built: " .. dir)
  ctx:defer(function() ctx:log("suite_dir torn down") end)
  return dir                                   -- prova.Fixture<string>
end)

-- file-scoped: one instance per test file; depends on the suite fixture.
local db = prova.fixture("db", Scope.File, function(ctx)
  local root = ctx:use(suite_dir)              -- root : string
  ctx:log("db opened under " .. root)
  ctx:defer(function() ctx:log("db closed") end)
  return { root = root, open_connections = 0 } -- prova.Fixture<table>
end)

-- test-scoped (default): fresh for every test. Mutates the shared file-scoped db so we can
-- observe that test-scoped teardown restores state while the file-scoped value persists.
local conn = prova.fixture("conn", Scope.Test, function(ctx)
  local database = ctx:use(db)                 -- database : table (same instance across tests in this file)
  database.open_connections = database.open_connections + 1
  ctx:defer(function() database.open_connections = database.open_connections - 1 end)
  return database.open_connections             -- prova.Fixture<number>
end)

prova.test("first test acquires connection #1", function(t)
  t:expect(t:use(conn)):equals(1)
end)

prova.test("second test also sees #1 — test-scoped conn was torn down and rebuilt", function(t)
  -- `db` is file-scoped, so it's the SAME instance as the previous test (not rebuilt),
  -- but `conn` is test-scoped: the previous test's defer decremented back to 0, so we're at 1 again.
  t:expect(t:use(conn)):equals(1)
end)

prova.test("the shared db root lives under the suite dir", function(t)
  local database = t:use(db)
  t:expect(database.root):contains(t:use(suite_dir))  -- same suite_dir instance, built once
end)
