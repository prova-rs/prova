--- The native `grpc` module against a REAL reflection-enabled gRPC server (moul/grpcbin) in an
--- ephemeral container. Run from the repo root: `prova examples/grpc_test.lua`. Requires docker;
--- skips gracefully where it is unavailable.
---
--- Note there are NO `.proto` files and no codegen here: the client discovers the server's schema at
--- runtime via gRPC Server Reflection, builds requests from plain Lua tables, and decodes replies
--- back to tables. grpcbin speaks the older v1alpha reflection protocol, exercising prova's fallback.

local server = prova.fixture("grpcbin", Scope.File, function(ctx)
  local c = ctx:manage(docker.run{
    image = "moul/grpcbin",
    ports = { 9000 },
    wait = { port = 9000, timeout = "60s" },
  })

  local addr = "127.0.0.1:" .. c:host_port(9000)
  grpc.wait_for(addr, { timeout = "30s" })
  local client = grpc.connect(addr)
  return client
end)

prova.group("grpc", { requires = { "docker" } }, function(g)
  g:test("unary call round-trips a request table to a response table", function(t)
    local client = t:use(server)
    local resp = client:call("hello.HelloService/SayHello", { greeting = "prova" })
    t:expect(resp.reply):equals("hello prova")
  end)

  g:test("echoes fields via the grpcbin dummy service", function(t)
    local client = t:use(server)
    local resp = client:call("grpcbin.GRPCBin/DummyUnary", { f_string = "roundtrip" })
    t:expect(resp.fString):equals("roundtrip")
  end)

  g:test("call_status surfaces gRPC error codes without raising", function(t)
    local client = t:use(server)
    -- SpecificError returns the requested status code (5 = NotFound) rather than a response.
    local res = client:call_status("grpcbin.GRPCBin/SpecificError", { code = 5, reason = "nope" })
    t:expect(res.ok):is_falsy()
    t:expect(res.code):equals("NotFound")
    t:expect(res.message):equals("nope")
  end)
end)
