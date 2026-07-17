-- PROOF 4 (containerized SUT) — THE PAYOFF. The system under test is BUILT from its own Dockerfile
-- and RUN in a container on the topology's network, wired to a resource through that resource's
-- network vantage. The SUT is not a special concept: it is a `prova.containerized` resource whose
-- image is *built* rather than *pulled*, so it inherits everything already proved — the topology
-- auto-join (Proof 3), the network vantage (Proof 2), the network primitive (Proof 1).
--
-- What this buys: the host needs NOTHING but Docker. No SDK, no JVM, no uv — the suite does not say
-- `requires = { "dotnet" }`, it says `requires = { "docker" }`, and the artifact under test is the
-- project's real production image rather than a host-built approximation.
--
-- The bar is end-to-end and black-box:
--   host test runner --HTTP over published port--> [SUT container] --postgres by DNS alias--> [db]
-- The runner never reaches inside either container; it drives the app over HTTP and cross-checks the
-- database over its own host vantage. Both vantages live at once, which is the whole point.
--
-- Run standalone: prova crates/prova-core/testdata/container_app.lua   (requires docker)

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
    return { close = function() end, exec = function(sql)
      return (container:run({ "env", "PGPASSWORD=prova",
        "psql", "-U", "prova", "-d", "prova", "-tAc", sql }):gsub("%s+$", ""))
    end }
  end,
}

-- The "project": a real HTTP service, in the smallest honest form. It reads DATABASE_URL from the
-- environment, connects to postgres with a REAL postgres client (psql, from its base image), and
-- serves the row count over HTTP. Standing in for a .NET/Java/Python service, it exercises the same
-- seams: build the image, wire by env, talk to the DB in-network, answer the host over HTTP.
local function write_project(dir)
  -- Per-connection handler: query on every request, so the response reflects LIVE database state
  -- rather than something baked at boot (which would make the count assertion below vacuous).
  fs.write(dir .. "/handler.sh", table.concat({
    "#!/bin/sh",
    "n=$(psql \"$DATABASE_URL\" -tAc 'select count(*) from widgets' 2>&1 | tr -d ' \\n')",
    "printf 'HTTP/1.1 200 OK\\r\\nContent-Length: %d\\r\\nConnection: close\\r\\n\\r\\n%s' \"${#n}\" \"$n\"",
  }, "\n"))
  -- The production Dockerfile, nested exactly where the archetypes ship theirs.
  fs.write(dir .. "/.platform/docker/local/Dockerfile", table.concat({
    "FROM postgres:16-alpine",
    "RUN apk add --no-cache socat",
    "COPY handler.sh /handler.sh",
    "RUN chmod +x /handler.sh",
    "EXPOSE 8080",
    "CMD [\"socat\", \"TCP-LISTEN:8080,fork,reuseaddr\", \"EXEC:/handler.sh\"]",
  }, "\n"))
end

local env = prova.topology("shop", function(ctx)
  local db = pg.container(ctx)                        -- auto-joins ctx.network, aliased "postgres"
  db.client.exec("create table widgets (id serial primary key)")

  -- The context is authored into a scope-managed tempdir purely because this proof ships its own
  -- project; a real suite points `context` at the repo and `dockerfile` at its checked-in path.
  local dir = ctx:tempdir()
  write_project(dir)

  -- THE SUT AS A RESOURCE: `build` where a pulled resource writes `image`. Everything else is the
  -- recipe grammar every plugin already uses.
  local app = prova.containerized{
    name = "app",
    build = { context = dir, dockerfile = ".platform/docker/local/Dockerfile" },
    port = 8080,
    timeout = "60s",
    -- Wired to the DB through its NETWORK vantage — alias + container port, the address that
    -- resolves from inside the network. The host vantage (127.0.0.1:<mapped>) would not.
    env = function(opts) return { DATABASE_URL = opts.database_url } end,
    url = function(hp) return "http://127.0.0.1:" .. hp end,
  }.container(ctx, { database_url = db.network.url })

  return { db = db, app = app }
end)

prova.test("a containerized SUT, built from its own Dockerfile, serves the host and reaches the DB by alias",
           { requires = { "docker" } }, function(t)
  local e = t:use(env)

  -- The SUT came out in the standard resource shape — it IS a resource.
  t:expect(e.app.host):equals("127.0.0.1")
  t:expect(e.app.port):never():equals(8080)          -- published on a random host port
  t:expect(e.app.url):equals("http://127.0.0.1:" .. e.app.port)
  t:expect(e.app.network.alias):equals("app")        -- auto-joined + auto-aliased by the topology
  t:expect(e.app.network.port):equals(8080)          -- in-network peers would use the container port

  -- THE PROOF, half one: the host runner drives the SUT over HTTP, and the SUT answers with data it
  -- could only have obtained by resolving `postgres` on the topology network and querying it.
  local res = http.get(e.app.url .. "/")
  t:expect(res.status):equals(200)
  t:expect(res.body):equals("0")

  -- THE PROOF, half two: change the world through the DB's HOST vantage, and the SUT — reading the
  -- same database over its NETWORK vantage — reports the change. Both vantages address one live
  -- resource, and the wiring is real rather than a fixture returning a constant.
  e.db.client.exec("insert into widgets default values")
  t:expect(http.get(e.app.url .. "/").body):equals("1")

  e.db.client.exec("insert into widgets default values")
  t:expect(http.get(e.app.url .. "/").body):equals("2")

  -- And the DB agrees from the host side — the same rows, seen from the other vantage.
  t:expect(e.db.client.exec("select count(*) from widgets")):equals("2")
end)
