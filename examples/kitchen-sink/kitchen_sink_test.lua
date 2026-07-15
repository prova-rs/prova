--- KITCHEN SINK — the comprehensive multi-service example. Two services in two languages, three
--- infrastructure dependencies, one end-to-end assertion chain:
---
---   inventory-producer (Rust)            audit-consumer (Python)
---   gRPC API · Postgres                  REST API · MySQL
---            └──── produces ──▶ Pulsar ◀──── consumes ────┘
---
--- A single test drives the whole topology: create an item over gRPC, assert it landed in the
--- producer's Postgres, watch the event flow through Pulsar into the consumer, read it back over
--- REST, and cross-check the consumer's MySQL.
---
--- This is the IDIOMATIC version — each dependency is one `X.container(ctx)` line, where postgres,
--- mysql, and pulsar are external plugins declared in this directory's prova.toml and attached with
--- `require(...)`. The same topology built from raw primitives (docker.run + container:run readiness
--- gates, no plugins) lives in ../kitchen_sink_primitives_test.lua.
---
--- Run from this directory (first run compiles the Rust service and installs Python deps):
---   cd examples/kitchen-sink && prova
--- requires docker + cargo + python3 (skips cleanly without them). Pulsar standalone dominates
--- startup (~60-90s); everything else rides in its shadow.

local postgres = require("postgres")
local mysql    = require("mysql")
local pulsar   = require("pulsar")

local TOPIC = "inventory-events"

-- Three real dependencies, one line each. The recipes gate on readiness that HOLDS (not just an
-- open socket) and tie every container and client to this fixture's teardown.
local infra = prova.fixture("infra", Scope.File, function(ctx)
  return {
    pg     = postgres.container(ctx, { user = "dev", password = "dev", database = "inventory" }),
    mysql  = mysql.container(ctx, { user = "dev", password = "dev", database = "audit" }),
    pulsar = pulsar.container(ctx),
  }
end)

-- The Rust half: gRPC in front, Postgres behind, Pulsar out the back.
local producer = prova.fixture("producer", Scope.File, function(ctx)
  local env = ctx:use(infra)
  local dir = "../fixtures/inventory-producer"

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
  -- The service connects to Postgres AND Pulsar before it serves, so a reflection answer proves
  -- the whole chain behind it.
  grpc.wait_for(addr, { timeout = "30s" })
  return { addr = addr }
end)

-- The Python half: Pulsar in, MySQL behind, REST in front.
local consumer = prova.fixture("consumer", Scope.File, function(ctx)
  local env = ctx:use(infra)
  local dir = "../fixtures/audit-consumer"

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
  -- HTTP up ⇒ the consumer's MySQL and Pulsar connections both succeeded.
  api:wait_for("/healthz", { timeout = "60s" })
  return { api = api }
end)

prova.group("kitchen sink: Rust gRPC producer → Pulsar → Python REST consumer",
            { requires = { "docker", "cargo", "python3" } }, function(g)

  g:test("an item created over gRPC flows through every tier", function(t)
    local env  = t:use(infra)
    local prod = t:use(producer)
    local cons = t:use(consumer)

    -- 1. Create through the producer's public API — the only door a real client has.
    local client = grpc.client(prod.addr)
    local created = client:call("inventory.v1.Inventory/CreateItem", { display_name = "widget" })
    t:expect(created.id, "created id"):gt(0)
    t:expect(created.display_name):equals("widget")

    -- 2. The write landed in the producer's own Postgres.
    t:expect(env.pg.client:query_value(
      "SELECT count(*) FROM items WHERE display_name = $1", { "widget" })):equals(1)

    -- 3. The event crossed Pulsar into the consumer; its REST API shows the audit. Consumption is
    --    asynchronous, so poll — gate on state, never sleep.
    local audits = prova.retry(function()
      local res = cons.api:get("/audits")
      if res.status == 200 then
        local body = res:json()
        if body.audits and #body.audits >= 1 then return body.audits end
      end
    end, { timeout = "30s", message = "audit event never arrived through Pulsar" })
    t:expect(audits[1].item_id):equals(created.id)
    t:expect(audits[1].display_name):equals("widget")

    -- 4. Cross-check the consumer's MySQL — the event's final resting place.
    t:expect(env.mysql.client:query_value(
      "SELECT count(*) FROM audits WHERE item_id = ? AND display_name = ?",
      { created.id, "widget" })):equals(1)
  end)

  g:test("the producer's list API reflects created items", function(t)
    local env  = t:use(infra)
    local prod = t:use(producer)

    local client = grpc.client(prod.addr)
    local created = client:call("inventory.v1.Inventory/CreateItem", { display_name = "sprocket" })

    -- Assert on OUR row, not on totals — the sibling test writes to the same shared Postgres, and
    -- independent tests must not depend on each other's side effects.
    local listed = client:call("inventory.v1.Inventory/ListItems", {})
    local found = false
    for _, item in ipairs(listed.items) do
      if item.id == created.id and item.display_name == "sprocket" then found = true end
    end
    t:expect(found, "created item present in ListItems"):is_true()
    t:expect(env.pg.client:query_value(
      "SELECT display_name FROM items WHERE id = $1", { created.id })):equals("sprocket")
  end)
end)
