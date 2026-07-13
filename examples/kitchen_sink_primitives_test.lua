--- PRIMITIVES companion to kitchen_sink_test.lua — the SAME two-service topology (Rust gRPC
--- producer · Pulsar · Python REST consumer), with all three infrastructure dependencies built by
--- hand instead of via the container recipes. Three different readiness shapes, deliberately:
---
---   postgres — socket gate, then retry a client until the connection HOLDS (first-boot restart)
---   mysql    — same shape, longer horizon (init takes tens of seconds)
---   pulsar   — a LOG gate ("messaging service is ready"): the broker announces readiness long
---              after its ports open
---
--- Read kitchen_sink_test.lua first; come back here when a dependency has no recipe or needs
--- custom wiring. Tagged "primitives".
---
---   prova examples/kitchen_sink_primitives_test.lua
--- requires docker + cargo + python3 (skips cleanly without them).

local TOPIC = "inventory-events"

local infra = prova.fixture("infra", Scope.File, function(ctx)
  -- Postgres, by hand: container + socket gate, URL from the mapped port, then gate on a
  -- connection that holds. (This is `postgres.container` unrolled — see also
  -- service_grpc_postgres_primitives_test.lua for the single-service walkthrough.)
  local pg_c = ctx:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_USER = "dev", POSTGRES_PASSWORD = "dev", POSTGRES_DB = "inventory" },
    ports = { 5432 },
    wait = { port = 5432, timeout = "60s" },
  })
  local pg_url = "postgres://dev:dev@127.0.0.1:" .. pg_c:host_port(5432) .. "/inventory"
  local pg = ctx:manage(prova.retry(function() return postgres.client(pg_url) end,
    { timeout = "30s", message = "postgres did not accept connections in time" }))

  -- MySQL, by hand: identical shape, but MySQL's first-boot init is slower and it also restarts —
  -- the retry horizon is the only thing that changes.
  local my_c = ctx:manage(docker.run{
    image = "mysql:8",
    env = { MYSQL_USER = "dev", MYSQL_PASSWORD = "dev", MYSQL_DATABASE = "audit",
            MYSQL_ROOT_PASSWORD = "root" },
    ports = { 3306 },
    wait = { port = 3306, timeout = "90s" },
  })
  local my_url = "mysql://dev:dev@127.0.0.1:" .. my_c:host_port(3306) .. "/audit"
  local my = ctx:manage(prova.retry(function() return mysql.client(my_url) end,
    { timeout = "90s", message = "mysql did not accept connections in time" }))

  -- Pulsar, by hand: a different readiness SHAPE. Standalone opens its ports well before it can
  -- serve, but it announces readiness in its logs — so the gate is `wait = { log = ... }`, and the
  -- client retry is just a belt over those suspenders.
  local pl_c = ctx:manage(docker.run{
    image = "apachepulsar/pulsar:3.3.1",
    command = "bin/pulsar standalone",
    ports = { 6650, 8080 },
    wait = { log = "messaging service is ready", timeout = "120s" },
  })
  local pl_url = "pulsar://127.0.0.1:" .. pl_c:host_port(6650)
  ctx:manage(prova.retry(function() return pulsar.client(pl_url) end,
    { timeout = "30s", message = "pulsar did not accept connections in time" }))

  return {
    pg = { client = pg, url = pg_url },
    mysql = { client = my, url = my_url },
    pulsar = { url = pl_url },
  }
end)

-- From here down the file is IDENTICAL to kitchen_sink_test.lua — the services neither know nor
-- care how their dependencies came to exist. That interchangeability is the point: a hand-rolled
-- provisioner that returns the standard { client, url, container } shape is indistinguishable
-- from a first-party recipe.

local producer = prova.fixture("producer", Scope.File, function(ctx)
  local env = ctx:use(infra)
  local dir = "examples/fixtures/inventory-producer"

  local build = shell.run("cargo build", { cwd = dir, timeout = "600s" })
  assert(build:ok(), "inventory-producer failed to build:\n" .. build.stderr)

  local port = net.free_port()
  ctx:manage(shell.spawn(dir .. "/target/debug/inventory-producer", {
    env = {
      DATABASE_URL = env.pg.url,
      PULSAR_URL   = env.pulsar.url,
      PULSAR_TOPIC = TOPIC,
      PORT         = tostring(port),
    },
  }))

  local addr = "127.0.0.1:" .. port
  grpc.wait_for(addr, { timeout = "30s" })
  return { addr = addr }
end)

local consumer = prova.fixture("consumer", Scope.File, function(ctx)
  local env = ctx:use(infra)
  local dir = "examples/fixtures/audit-consumer"

  local venv = ctx:tempdir()
  local setup = shell.run(
    "python3 -m venv " .. venv .. " && " .. venv .. "/bin/pip install -q -r requirements.txt",
    { cwd = dir, timeout = "300s" })
  assert(setup:ok(), "audit-consumer venv setup failed:\n" .. setup.stderr)

  local port = net.free_port()
  ctx:manage(shell.spawn(venv .. "/bin/python main.py", {
    cwd = dir,
    env = {
      DATABASE_URL = env.mysql.url,
      PULSAR_URL   = env.pulsar.url,
      PULSAR_TOPIC = TOPIC,
      PORT         = tostring(port),
    },
  }))

  local api = http.client{ base_url = "http://127.0.0.1:" .. port }
  api:wait_for("/healthz", { timeout = "60s" })
  return { api = api }
end)

prova.group("kitchen sink from primitives: Rust gRPC producer → Pulsar → Python REST consumer",
            { requires = { "docker", "cargo", "python3" }, tags = { "primitives" } }, function(g)

  g:test("an item created over gRPC flows through every tier", function(t)
    local env  = t:use(infra)
    local prod = t:use(producer)
    local cons = t:use(consumer)

    local client = grpc.client(prod.addr)
    local created = client:call("inventory.v1.Inventory/CreateItem", { display_name = "widget" })
    t:expect(created.id, "created id"):gt(0)

    t:expect(env.pg.client:query_value(
      "SELECT count(*) FROM items WHERE display_name = $1", { "widget" })):equals(1)

    local audits = prova.retry(function()
      local res = cons.api:get("/audits")
      if res.status == 200 then
        local body = res:json()
        if body.audits and #body.audits >= 1 then return body.audits end
      end
    end, { timeout = "30s", message = "audit event never arrived through Pulsar" })
    t:expect(audits[1].item_id):equals(created.id)
    t:expect(audits[1].display_name):equals("widget")

    t:expect(env.mysql.client:query_value(
      "SELECT count(*) FROM audits WHERE item_id = ? AND display_name = ?",
      { created.id, "widget" })):equals(1)
  end)
end)
