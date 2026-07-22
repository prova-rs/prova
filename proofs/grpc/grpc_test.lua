-- Dogfoods the native `grpc` module against a real reflection-enabled server (moul/grpcbin) in an
-- ephemeral container: no `.proto` files, no codegen — the client discovers the schema at runtime
-- via gRPC Server Reflection (grpcbin speaks v1alpha, exercising prova's fallback), builds requests
-- from Lua tables, and decodes replies back to tables.

local server = prova.fixture("grpcbin", Scope.File, function(ctx)
  local c = ctx:manage(docker.run{
    image = "moul/grpcbin",
    ports = { 9000 },
    wait = { port = 9000, timeout = "60s" },
  })

  local addr = "127.0.0.1:" .. c:host_port(9000)
  grpc.wait_for(addr, { timeout = "30s" })
  return grpc.client(addr)
end)

prova.group("grpc", { requires = { "docker" } }, function(g)
  g:test("unary call round-trips a request table to a response table", function(t)
    local client = t:use(server)
    local resp = client:call("hello.HelloService/SayHello", { greeting = "prova" })
    t:expect(resp.reply):equals("hello prova")
  end)

  g:test("echoes fields via the grpcbin dummy service", function(t)
    local client = t:use(server)
    -- Requests and responses share the same proto (snake_case) field names.
    local resp = client:call("grpcbin.GRPCBin/DummyUnary", { f_string = "roundtrip" })
    t:expect(resp.f_string):equals("roundtrip")
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

-- Reflection dependency-chasing regression: tonic-based servers answer
-- `file_containing_symbol` with ONLY the named file; imported files (the
-- well-known types here) must be fetched with `file_by_filename` follow-ups
-- and added to the pool in dependency order. Before that chase existed, this
-- client failed with "imported file 'google/protobuf/empty.proto' has not
-- been added". No docker: the mock IS a tonic server.
prova.test("client chases imported descriptor files through reflection", function(t)
	local dir = t:tempdir()
	fs.write(dir .. "/ping.proto", [[
syntax = "proto3";
package ping;
import "google/protobuf/empty.proto";
service Ping {
  rpc Poke (google.protobuf.Empty) returns (Pong);
}
message Pong {
  string note = 1;
}
]])
	local mock = grpc.mock(t, { proto = dir .. "/ping.proto" })
	mock:on({ method = "ping.Ping/Poke" }):reply({ response = { note = "poked" } })
	local client = grpc.client(mock.host .. ":" .. mock.port)
	local resp = client:call("ping.Ping/Poke", {})
	t:expect(resp.note):equals("poked")
end)
