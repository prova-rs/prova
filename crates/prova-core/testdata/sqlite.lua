-- SQLite (embedded, no server): proves the general query API. Postgres/MySQL use the SAME calls —
-- only the namespace and URL scheme differ. A test-scoped fixture gives each test a fresh database.
local conn = prova.fixture("conn", Scope.Test, function(ctx)
  local path = ctx:tempdir() .. "/test.db"
  local c = sqlite.client("sqlite://" .. path .. "?mode=rwc")
  c:execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active BOOLEAN)")
  ctx:defer(function() c:close() end)
  return c
end)

prova.test("execute reports rows affected; query returns column-mapped rows", function(t)
  local c = t:use(conn)
  local n = c:execute("INSERT INTO users (id, name, score, active) VALUES (?, ?, ?, ?)", { 1, "alice", 9.5, true })
  t:expect(n):equals(1)
  c:execute("INSERT INTO users (id, name, score, active) VALUES (?, ?, ?, ?)", { 2, "bob", 3.0, false })

  local rows = c:query("SELECT id, name, score FROM users ORDER BY id")
  t:expect(#rows):equals(2)
  t:expect(rows[1].id):equals(1)
  t:expect(rows[1].name):equals("alice")
  t:expect(rows[1].score):equals(9.5)
  t:expect(rows[2].name):equals("bob")
end)

prova.test("query_value returns a scalar; params filter; missing is nil", function(t)
  local c = t:use(conn)
  c:execute("INSERT INTO users (id, name, score, active) VALUES (?, ?, ?, ?)", { 1, "carol", 7.0, true })
  c:execute("INSERT INTO users (id, name, score, active) VALUES (?, ?, ?, ?)", { 2, "dave", 4.0, false })
  t:expect(c:query_value("SELECT count(*) FROM users")):equals(2)
  t:expect(c:query_value("SELECT name FROM users WHERE id = ?", { 1 })):equals("carol")
  t:expect(c:query_value("SELECT name FROM users WHERE id = ?", { 999 })):is_nil()
end)
