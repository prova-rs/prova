-- PROOF 3 (networked topologies) — the CONVENIENCE. A prova.topology creates and manages ONE
-- user-defined network; resources provisioned in its factory auto-join it, aliased by name, with no
-- docker.network() call by the author. This is pure sugar over the primitives Proofs 1 & 2 proved:
-- `ctx.network` IS a docker.network() the topology made and manages, and auto-join IS the
-- `network`/`alias` opts filled in for you — you can still override the alias, opt out, or hand-wire
-- with docker.run. The convenience never removes the primitive.
--
-- Run standalone: prova crates/prova-core/testdata/topology_network.lua   (requires docker)

local pg = prova.containerized{
  name = "postgres", image = "postgres", tag = "16-alpine", port = 5432, timeout = "60s",
  env = function(opts)
    return { POSTGRES_USER = "prova", POSTGRES_PASSWORD = "prova", POSTGRES_DB = "prova" }
  end,
  url = function(hp, opts)
    return string.format("postgres://prova:prova@127.0.0.1:%d/prova", hp)
  end,
  client = function(url, opts, container)
    container:run({ "env", "PGPASSWORD=prova", "psql", "-U", "prova", "-d", "prova", "-tAc", "select 1" })
    return { close = function() end }
  end,
}

-- No docker.network() authored anywhere: the topology provides the ambient network.
local env = prova.topology("shop", function(ctx)
  -- The topology's network is on ctx.network — a real managed docker network handle.
  local db = pg.container(ctx)                    -- auto-joins ctx.network, alias defaults to "postgres"

  -- A probe on the SAME auto-network reaches db by its auto-alias — the reachability bar. It uses
  -- the primitive (docker.run{ network = ctx.network }) directly: the convenience and the primitive
  -- share one network.
  local probe = ctx:manage(docker.run{
    image = "postgres:16-alpine", network = ctx.network, command = "sleep 120",
  })

  return { db = db, probe = probe, net_name = ctx.network.name }
end)

prova.test("a topology auto-wires one shared network; resources join it aliased by name",
           { requires = { "docker" } }, function(t)
  local e = t:use(env)

  -- ctx.network was a real, managed user-defined network.
  t:expect(e.net_name, "the topology's ambient network"):matches("prova")

  -- The resource auto-joined that network, aliased by its recipe name — no opts authored.
  t:expect(e.db.network.alias):equals("postgres")
  t:expect(e.db.network.host):equals("postgres")
  t:expect(e.db.network.port):equals(5432)

  -- THE PROOF: reachability over the AUTO network by the AUTO alias, with no docker.network() and no
  -- network/alias opts written by the author — the topology wired it.
  local out = e.probe:run({
    "env", "PGPASSWORD=prova",
    "psql", "-h", "postgres", "-U", "prova", "-d", "prova", "-tAc", "select 42",
  })
  t:expect(out):contains("42")
end)
