-- A mock handler that raises fails the scope that owns the mock.
--
-- Closes decision 3's second rule in docs/plans/mocks.md. A handler runs on a server task, outside
-- any test's stack, so a raise there has nowhere to land: before this it answered 500, recorded
-- `error` in the journal, and was otherwise invisible. A system under test with a retry or a
-- fallback would swallow the 500 and the suite would go green with a broken handler — reporting the
-- dependency as flaky when the bug is ours.
--
-- It rides the teardown machinery rather than inventing anything: `stop()` raises, `ctx:manage`
-- calls `stop()` at scope end, and a raising teardown is now its own reported leaf
-- (`<scope> ⟶ teardown`, see testdata/teardown_errors.lua). That is why the handler-error gap was
-- NOT the small fix it was first called: nothing was listening.
--
-- Read the counts in tests/mock_handler_errors.rs — the failures here are the assertion.

--------------------------------------------------------------------------------------------
-- Strict by default: the test body passes, and the scope fails at teardown.
--------------------------------------------------------------------------------------------
prova.test("a raising handler fails the scope, even when the SUT swallows the 500", function(t)
  local m = http.mock(t)
  m:on{ path = "/boom" }:reply(function() error("handler blew up") end)

  -- A SUT with a fallback: it tolerates the 500 and carries on. Nothing here asserts on the
  -- journal, which is exactly the suite that used to go green over a broken handler.
  local res = http.get(m.url .. "/boom")
  t:expect(res.status):equals(500)
end)

--------------------------------------------------------------------------------------------
-- The opt-out, for a test whose subject IS the error path. Explicit, because the alternative is
-- magic — "did the test happen to call received()?" is not a contract anyone can read.
--------------------------------------------------------------------------------------------
prova.test("allow_handler_errors makes the error path assertable", function(t)
  local m = http.mock(t, { allow_handler_errors = true })
  m:on{ path = "/boom" }:reply(function() error("deliberate") end)

  local res = http.get(m.url .. "/boom")
  t:expect(res.status):equals(500)
  t:expect(m:received()[1].error):contains("deliberate")
end)

--------------------------------------------------------------------------------------------
-- A handler that returns the wrong shape is a handler error too — the same class of "our bug
-- wearing the dependency's clothes", so it must not be quieter.
--------------------------------------------------------------------------------------------
prova.test("a handler returning a non-table also fails the scope", function(t)
  local m = http.mock(t)
  m:on{ path = "/bad" }:reply(function() return "not a table" end)
  t:expect(http.get(m.url .. "/bad").status):equals(500)
end)

--------------------------------------------------------------------------------------------
-- The negative control. Without it, every test above would pass just as well against a mock that
-- failed its scope unconditionally.
--------------------------------------------------------------------------------------------
prova.test("a healthy mock does not fail its scope", function(t)
  local m = http.mock(t)
  m:on{ path = "/ok" }:reply{ status = 200, body = "fine" }
  t:expect(http.get(m.url .. "/ok").body):equals("fine")
end)

--------------------------------------------------------------------------------------------
-- Same rule on the grpc facet: one behaviour, not two.
--------------------------------------------------------------------------------------------
local proto = prova.fixture("proto", Scope.File, function(ctx)
  local path = ctx:tempdir() .. "/p.proto"
  fs.write(path, [[
syntax = "proto3";
package p;
service S { rpc Go (Req) returns (Rep); }
message Req { string a = 1; }
message Rep { string b = 1; }
]])
  return path
end)

prova.test("a raising grpc handler fails the scope", function(t)
  local m = grpc.mock(t, { proto = t:use(proto) })
  m:on{ method = "p.S/Go" }:reply(function() error("grpc handler blew up") end)

  local c = grpc.client(m.url)
  t:expect(c:call_status("p.S/Go", { a = "x" }).code):equals("Internal")
end)

prova.test("allow_handler_errors works on grpc too", function(t)
  local m = grpc.mock(t, { proto = t:use(proto), allow_handler_errors = true })
  m:on{ method = "p.S/Go" }:reply(function() error("deliberate grpc") end)

  local c = grpc.client(m.url)
  t:expect(c:call_status("p.S/Go", { a = "x" }).ok):is_false()
  t:expect(m:received()[1].error):contains("deliberate grpc")
end)
