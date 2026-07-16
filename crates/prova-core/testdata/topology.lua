-- A topology is a fixture that is also addressable by name (for `prova up`). In test mode it is used
-- exactly like any fixture, and defaults to Scope.File — provisioned once, shared across the file.

local built = 0
local env = prova.topology("web", function(ctx)
  built = built + 1
  return {
    app = { url = "http://127.0.0.1:8080" },
    db = { url = "postgres://dev@127.0.0.1/app" },
  }
end)

prova.test("a topology is usable as a fixture", function(t)
  local e = t:use(env)
  t:expect(e.app.url):equals("http://127.0.0.1:8080")
  t:expect(e.db.url):matches("^postgres://")
end)

prova.test("a topology is File-scoped: built once, shared across the file", function(t)
  local e = t:use(env)
  t:expect(e.app.url):equals("http://127.0.0.1:8080")
  t:expect(built):equals(1) -- the sibling test already built it; File scope returns the same instance
end)
