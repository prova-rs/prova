--- CAPSTONE — the North Star hard tier, against a REAL p6m archetype. Render a gRPC service with
--- Postgres persistence, build it, provision an ephemeral Postgres, boot the service wired to it, and
--- drive its gRPC API while cross-checking the same database — the tier a declarative harness (the
--- pytest manifest) structurally cannot express. Run it with:
---   cd examples/service-grpc-postgres && prova
--- requires docker + cargo (skips cleanly without either); first run clones libs + downloads crates.
---
--- The database is the external `postgres` plugin, declared in this directory's prova.toml and
--- attached with `require("postgres")`. `require("postgres").container(ctx, ...)` provisions it in one
--- line — provision + readiness + managed teardown + a docker-exec client. The same integration built
--- from raw primitives (docker.run + container:run readiness gates) lives in
--- ../service_grpc_postgres_primitives.lua — read that when you need a resource with no plugin.
---
--- NOTE (why this matters): the archetype today is a SCAFFOLD — its gRPC methods return
--- `Unimplemented` and its migration is empty. prova *running* the service is exactly what exposes
--- that "renders + compiles" was hiding a hollow service. As the archetype grows real CRUD, the
--- assertions below graduate from "Unimplemented" to real persisted state (Create → row → Get).

local postgres = require("postgres")

local ANSWERS = {
  author_name = "Test Author", author_email = "test@example.com",
  org_name = "acme", solution_name = "platform",
  prefix_name = "inventory", suffix_name = "Service", image_registry = "ghcr.io/acme",
  persistence = "PostgreSQL",
}

-- Render once (headless), shared across the suite.
local project = prova.fixture("project", Scope.File, function(ctx)
  return archetect.render{
    source = "https://github.com/p6m-archetypes/rust-grpc-service-archetype.git#dev",
    answers = ANSWERS,
    destination = ctx:tempdir(),
    defaults = true,
  }
end)

-- Provision Postgres, build the service, and boot it wired to the container. Returns the gRPC address
-- and the DB client (so tests can cross-check the very database the service is using).
local service = prova.fixture("service", Scope.File, function(ctx)
  local dir = ctx:use(project):dir("inventory-service").path

  -- One line: container, readiness (a connection that HOLDS, not just an open port), managed
  -- teardown. `pg.url` is what we inject into the service; `pg.client` is ours to cross-check with.
  local pg = postgres.container(ctx, { user = "dev", password = "dev", database = "inventory_service" })
  local db_url = pg.url

  local build = shell.run("cargo build", { cwd = dir, timeout = "600s" })
  assert(build:ok(), "service failed to build:\n" .. build.stderr)

  -- Boot the built binary wired to Postgres via the service's own env config (figment APP_* / __).
  local port = net.free_port()
  ctx:manage(shell.spawn(dir .. "/target/debug/inventory-service", {
    cwd = dir,
    env = {
      APP_PERSISTENCE__URL = db_url,
      APP_SERVER__PORT = tostring(port),
      APP_SERVER__MANAGEMENT_PORT = tostring(port + 1),
    },
  }))

  local addr = "127.0.0.1:" .. port
  grpc.wait_for(addr, { timeout = "30s" })  -- the service only answers if it connected to Postgres
  return { addr = addr, db = pg.client }
end)

prova.group("inventory gRPC service (Postgres)", { requires = { "docker", "cargo" } }, function(g)
  g:test("boots against real Postgres and serves its gRPC API", function(t)
    local svc = t:use(service)
    local client = grpc.client(svc.addr)
    -- Reaching a reflection-built client at all proves the service booted — which required a live
    -- Postgres connection. The method is reachable; today the scaffold answers Unimplemented.
    local res = client:call_status("inventory_service.InventoryService/CreateInventory",
                                   { display_name = "widget" })
    t:expect(res.code):equals("Unimplemented")  -- becomes "Ok" as real CRUD lands in the archetype
  end)

  g:test("ran its migrations against that same Postgres", function(t)
    local svc = t:use(service)
    -- The plugin's docker-exec client execs `psql` inside the very container the service is wired to —
    -- cross-service state assertion with no extra connection ceremony.
    t:expect(svc.db:query_value("SELECT count(*) FROM _sqlx_migrations WHERE success")):gte(1)
  end)
end)
