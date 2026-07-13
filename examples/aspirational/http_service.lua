--- Example: render a service archetype, build it, boot it, and probe it over HTTP.
---
--- Demonstrates: suite-scoped + parametrized fixtures, a fixture that starts and stops a
--- long-running process via ctx:defer, http.wait_for for boot-then-probe, and table-driven
--- tests. This is the black-box acceptance layer the framework is built for.

-- Parametrized suite fixture: the whole file's tests run once per toolchain.
local toolchain = prova.fixture("toolchain", "suite", function(ctx)
  return { name = ctx:param() }
end, { params = { "stable" } })

-- Render + build once per suite.
local built_service = prova.fixture("built_service", "suite", function(ctx)
  local tc = ctx:use(toolchain)
  local out = archetect.render{
    source = "https://github.com/archetect/archetype-rust-service-tonic-workspace.git",
    answers = { project_name = "orders", port = 8080 },
    destination = ctx:tempdir(),
    defaults = true,
  }
  local build = shell.run("cargo +" .. tc.name .. " build --release", { cwd = out.path, timeout = "300s" })
  assert(build:ok(), "service failed to build:\n" .. build.stderr)
  return out
end)

-- Boot the built binary, wait for health, tear it down at suite end.
local running_service = prova.fixture("running_service", "suite", function(ctx)
  local svc = ctx:use(built_service)
  local proc = shell.run("./target/release/orders &", { cwd = svc.path })  -- illustrative; real API: shell.spawn
  ctx:defer(function() shell.run("pkill -f target/release/orders") end)
  http.wait_for("http://localhost:8080/health", { status = 200, timeout = "30s", every = "500ms" })
  return { base = "http://localhost:8080" }
end)

prova.test("health endpoint is green", function(t)
  local svc = t:use(running_service)
  local res = http.get(svc.base .. "/health")
  t:expect(res.status):equals(200)
  t:expect(res:json().status):equals("ok")
end)

prova.test_each("rejects bad input on {route}", {
  { route = "/orders", payload = {} },
  { route = "/orders", payload = { quantity = -1 } },
}, function(t, case)
  local svc = t:use(running_service)
  local res = http.post(svc.base .. case.route, { json = case.payload })
  t:expect(res.status):is_one_of({ 400, 422 })
end)
