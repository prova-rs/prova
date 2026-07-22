-- Dogfoods the fixture model: the three scopes cache correctly (Suite built once, File once per file,
-- Test every test), fixtures depend on fixtures, and ctx:defer runs teardown (observable in-scope).
local counter = prova.fixture("counter", Scope.Test, function(ctx)
  local n = { value = 1 }
  ctx:defer(function() n.value = 0 end)   -- teardown (observable only within scope)
  return n
end)

prova.test("a Scope.Test fixture is built fresh", function(t)
  t:expect(t:use(counter).value):equals(1)
end)

prova.test("...and rebuilt for the next test, not carried over", function(t)
  t:expect(t:use(counter).value):equals(1)   -- 1 again, not mutated from the prior test
end)

-- Scope.Suite → built once for the whole run; Scope.File → once per file, depending on the suite one.
local suite_dir = prova.fixture("suite_dir", Scope.Suite, function(ctx)
  return ctx:tempdir()
end)
local db = prova.fixture("db", Scope.File, function(ctx)
  return { root = ctx:use(suite_dir), open = 0 }   -- fixture-to-fixture dependency
end)
-- Test-scoped: fresh per test; mutates the file-scoped db and undoes it on teardown.
local conn = prova.fixture("conn", Scope.Test, function(ctx)
  local d = ctx:use(db)                                -- same db instance across tests in this file
  d.open = d.open + 1
  ctx:defer(function() d.open = d.open - 1 end)
  return d.open
end)

prova.test("Test-scoped is per test, File-scoped persists: connection count returns to #1", function(t)
  t:expect(t:use(conn)):equals(1)
end)
prova.test("...still #1 — the prior test's Test-scoped teardown decremented the shared db back", function(t)
  t:expect(t:use(conn)):equals(1)
end)
prova.test("the file-scoped db lives under the one Suite-scoped dir", function(t)
  t:expect(t:use(db).root):contains(t:use(suite_dir))   -- same suite_dir, built once
end)
