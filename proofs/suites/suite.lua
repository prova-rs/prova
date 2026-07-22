-- Dogfoods a SUITE: a directory with a `suite.lua` is ONE suite whose `*_test.lua` files share this
-- state — a `Scope.Suite` fixture is built once and shared across every file. Here that is an
-- in-memory store: file `a` writes it, file `b` reads back what `a` wrote, because it is the SAME
-- instance (a `Scope.File` fixture would be rebuilt per file, and `b` would see nothing).
suite.config{ name = "orders" }

prova.fixture("store", Scope.Suite, function(ctx)
  return { orders = {} } -- one table, shared by every file in the suite
end)
