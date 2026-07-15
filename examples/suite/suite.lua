--- Suite setup for `examples/suite/` — runs once, in the suite's shared state, before the test files.
--- A directory with a `suite.lua` is ONE suite: its `*_test.lua` files share this state, so a
--- `Scope.Suite` fixture is provisioned once and shared across them. Run: `prova examples/suite`.
suite.config{ name = "orders", requires = { "docker" } }

-- ONE Postgres for the whole suite — provisioned once, torn down once, shared by every file below.
prova.fixture("db", Scope.Suite, function(ctx)
  return require("postgres").container(ctx, { database = "orders" }).client
end)
