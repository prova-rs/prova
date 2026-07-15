--- PRIMITIVES companion to examples/service-grpc-postgres/ — the SAME integration, with the database
--- built by hand instead of via the external `postgres` plugin. Read this when you need a dependency
--- no plugin covers: everything `require("postgres").container` does is a few primitives you already
--- have — `docker.run` (+ a `wait` gate), `container:run` (drive the CLI in the image), and
--- `prova.retry` / `ctx:manage`. No plugin, so no prova.toml — run it directly:
---   prova examples/service_grpc_postgres_primitives_test.lua
--- requires docker + cargo (skips cleanly without either). Tagged "primitives".

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

-- A tiny docker-exec psql helper — what the postgres plugin wraps. Runs a query inside the container
-- (no shell, no quoting) and returns the trimmed scalar.
local function psql(container, sql)
  return (container:run({
    "env", "PGPASSWORD=dev", "psql", "-U", "dev", "-d", "inventory_service", "-tAc", sql,
  }):gsub("%s+$", ""))
end

local service = prova.fixture("service", Scope.File, function(ctx)
  local dir = ctx:use(project):dir("inventory-service").path

  -- What `require("postgres").container(ctx, opts)` does for you, step by step:

  -- 1. Start the container. `ports = { 5432 }` publishes to a RANDOM host port (parallel runs never
  --    collide); `wait = { port = ... }` gates on a listening socket. `ctx:manage` ties removal to
  --    this fixture's teardown — pass or fail, nothing leaks.
  local pg = ctx:manage(docker.run{
    image = "postgres:16-alpine",
    env = { POSTGRES_USER = "dev", POSTGRES_PASSWORD = "dev", POSTGRES_DB = "inventory_service" },
    ports = { 5432 },
    wait = { port = 5432, timeout = "60s" },
  })

  -- 2. Recover the mapped port and build the URL the app under test will be given.
  local db_url = "postgres://dev:dev@127.0.0.1:" .. pg:host_port(5432) .. "/inventory_service"

  -- 3. Gate on REAL readiness, not a socket. Postgres restarts once during first-boot init, so a
  --    listening port is not yet a database. Retry a real query (via `container:run`) until it HOLDS —
  --    the service we boot next connects exactly once and exits on failure.
  prova.retry(function() psql(pg, "SELECT 1"); return true end,
    { timeout = "30s", message = "postgres did not accept connections in time" })

  local build = shell.run("cargo build", { cwd = dir, timeout = "600s" })
  assert(build:ok(), "service failed to build:\n" .. build.stderr)

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
  return { addr = addr, container = pg }
end)

prova.group("inventory gRPC service (Postgres, from primitives)",
            { requires = { "docker", "cargo" } }, function(g)
  g:test("boots against real Postgres and serves its gRPC API", function(t)
    local svc = t:use(service)
    local client = grpc.client(svc.addr)
    local res = client:call_status("inventory_service.InventoryService/CreateInventory",
                                   { display_name = "widget" })
    t:expect(res.code):equals("Unimplemented")  -- becomes "Ok" as real CRUD lands in the archetype
  end)

  g:test("ran its migrations against that same Postgres", function(t)
    local svc = t:use(service)
    -- Cross-check the very database the service is wired to, by execing psql in its container.
    t:expect(psql(svc.container, "SELECT count(*) FROM _sqlx_migrations WHERE success")):gte(1)
  end)
end)
