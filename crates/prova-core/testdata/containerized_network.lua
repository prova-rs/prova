-- PROOF 2 (networked topologies) — the resource vantage. A `prova.containerized` resource created
-- on a network exposes a second addressing vantage: `resource.network = { url, host, port, alias }`,
-- the alias + CONTAINER port an in-network consumer (a containerized SUT) uses — distinct from the
-- host vantage (`url`/`host`/`port`, the mapped port the test runner uses). Both are live: the host
-- client queries over the mapped port, and a sibling container reaches the resource by its alias.
--
-- Run standalone: prova crates/prova-core/testdata/containerized_network.lua   (requires docker)

-- A postgres-shaped recipe, exactly as a plugin authors one (host url hardcodes 127.0.0.1).
local pg = prova.containerized{
  name = "postgres", image = "postgres", tag = "16-alpine", port = 5432, timeout = "60s",
  env = function(opts)
    return { POSTGRES_USER = "prova", POSTGRES_PASSWORD = "prova", POSTGRES_DB = "prova" }
  end,
  url = function(hp, opts)
    return string.format("postgres://prova:prova@127.0.0.1:%d/prova", hp)
  end,
  client = function(url, opts, container)
    -- docker-exec readiness: a query that HOLDS (postgres restarts once at init).
    container:run({ "env", "PGPASSWORD=prova", "psql", "-U", "prova", "-d", "prova", "-tAc", "select 1" })
    return { close = function() end, exec = function(sql)
      return (container:run({ "env", "PGPASSWORD=prova",
        "psql", "-U", "prova", "-d", "prova", "-tAc", sql }):gsub("%s+$", ""))
    end }
  end,
}

prova.test("a containerized resource on a network exposes host and network vantages",
           { requires = { "docker" } }, function(t)
  local net = t:manage(docker.network())

  -- Provision the resource ON the network with an alias. The recipe returns the standard shape
  -- plus the new `network` vantage.
  local db = pg.container(t, { network = net, alias = "db" })

  -- Host vantage (unchanged): the test runner's client works over the mapped port.
  t:expect(db.host):equals("127.0.0.1")
  t:expect(db.port):never():equals(5432)
  t:expect(db.url):contains("127.0.0.1:" .. db.port)
  t:expect(db.client.exec("select 42")):equals("42")

  -- Network vantage: alias + the CONTAINER port, and a url rewritten to that authority.
  t:expect(db.network.alias):equals("db")
  t:expect(db.network.host):equals("db")
  t:expect(db.network.port):equals(5432)
  t:expect(db.network.url):equals("postgres://prova:prova@db:5432/prova")

  -- The proof the network vantage is REAL: a sibling container connects using it.
  local client = t:manage(docker.run{ image = "postgres:16-alpine", network = net, command = "sleep 120" })
  local out = client:run({
    "env", "PGPASSWORD=prova",
    "psql", "-h", db.network.host, "-p", tostring(db.network.port), "-U", "prova", "-d", "prova",
    "-tAc", "select 42",
  })
  t:expect(out):contains("42")
end)
