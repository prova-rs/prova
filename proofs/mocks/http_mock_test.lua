-- Dogfoods the http.mock facet: stub, drive, assert on the recorded interaction.
prova.test("http.mock stubs a response and records the call", function(t)
  local m = http.mock(t)
  m:on{ method = "GET", path = "/health" }:reply{ status = 200, json = { ok = true } }

  local res = http.get(m.url .. "/health")
  t:expect(res.status):equals(200)
  t:expect(res:json().ok):is_true()
  t:expect(m:received{ path = "/health" }):has_length(1)
end)

prova.test("a Lua handler computes the response from the request", function(t)
  local m = http.mock(t)
  m:on{ method = "POST", route = "/echo/:word" }:reply(function(req)
    return { status = 200, json = { echoed = req.params.word } }
  end)
  t:expect(http.post(m.url .. "/echo/prova"):json().echoed):equals("prova")
end)
