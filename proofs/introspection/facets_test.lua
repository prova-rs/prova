--- The namespacing grammar, held to the shipped surface (docs/design/namespacing.md). The grammar
--- is a contract about SHAPE: fixed facet names within a namespace, facets present only where they
--- mean something, and the pre-grammar spellings gone. Most resource namespaces (postgres, redis,
--- kafka, s3…) live in their own packages now, so what this file pins is the grammar as the
--- in-tree namespaces embody it — the breadth lives downstream in each package's own suite.

prova.test("the fixed facet names resolve where the grammar says they exist",
  { covers = "docs/design/namespacing.md#facet-grammar" }, function(t)
  -- `client` attaches — every service/protocol namespace carries it.
  t:expect(type(sqlite.client)):equals("function")
  t:expect(type(http.client)):equals("function")
  t:expect(type(grpc.client)):equals("function")
  t:expect(type(graphql.client)):equals("function")
  -- `wait_for` — readiness polling, where the protocol supports a cheap probe.
  t:expect(type(http.wait_for)):equals("function")
  t:expect(type(grpc.wait_for)):equals("function")
  -- `mock` — provision a fake, where a protocol can be served in-process.
  t:expect(type(http.mock)):equals("function")
  t:expect(type(grpc.mock)):equals("function")
end)

-- Deliberately dynamic lookups: these tests assert ABSENCE, and a literal `sqlite.container`
-- would (rightly) read as an undefined-field mistake to the IDE. The indirection is the point:
-- the field must not exist, in the stubs or in the runtime.
local function facet(ns, name)
  return ns[name]
end

prova.test("facets are optional per namespace — absent exactly where they make no sense",
  { covers = "docs/design/namespacing.md#facets-are-optional" }, function(t)
  -- Nothing to provision: sqlite is a file, and http/grpc/graphql are protocol namespaces. A
  -- `container` on any of them would be grammar noise — its absence is part of the contract.
  t:expect(facet(sqlite, "container")):is_nil()
  t:expect(facet(http, "container")):is_nil()
  t:expect(facet(grpc, "container")):is_nil()
  t:expect(facet(graphql, "container")):is_nil()
  -- And `mock` only where a protocol can be served: you would never mock sqlite; you would run it.
  t:expect(facet(sqlite, "mock")):is_nil()
end)

prova.test("the pre-grammar spellings stay gone",
  { covers = "docs/design/namespacing.md#grammar-replaced-old-spellings" }, function(t)
  -- The old grouping module: `db.connect` dispatched on URL scheme. The grammar replaced the
  -- module wholesale — technology-first namespaces, `client` to attach — so `db` must carry no
  -- value at all, and no in-tree namespace may grow a `connect` back.
  t:expect(rawget(_G, "db")):is_nil()
  t:expect(facet(sqlite, "connect")):is_nil()
  t:expect(facet(http, "connect")):is_nil()
  t:expect(facet(grpc, "connect")):is_nil()
  t:expect(facet(graphql, "connect")):is_nil()
end)
