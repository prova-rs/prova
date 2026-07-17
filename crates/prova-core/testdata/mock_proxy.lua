-- http.mock passthrough / record / replay — the observe dial.
--
-- A proxy is not a second concept: it is a mock whose *unmatched* requests are forwarded instead of
-- 404'd. Same object, same stubs, same journal, one option. That is the whole design.
--
-- Observe mode is the only mode that is purely additive to the black-box thesis: the dependency is
-- real, the traffic is real, and we only watched. It answers the question a real dependency cannot —
-- not "did it work", but "what did we say".
--
-- The "real service" here is another http.mock. That is not a shortcut, it is the point: these
-- proofs need no network and no docker, and a mock that can stand in for the real service *for its
-- own proxy* is the same claim the facet makes to a SUT.

prova.test("passthrough forwards to the real service, and records the exchange", function(t)
  local real = http.mock(t)
  real:on{ path = "/v1/price/A1" }:reply{ status = 200, json = { cents = 999 } }

  local proxy = http.mock(t, { passthrough = real.url })

  local res = http.get(proxy.url .. "/v1/price/A1")
  t:expect(res.status):equals(200)
  t:expect(res:json().cents):equals(999)      -- the REAL service composed this answer

  t:expect(real:received()):has_length(1)     -- it genuinely went through, not around
  t:expect(proxy:received()):has_length(1)    -- and we saw it on the way
  t:expect(proxy:received()[1].source):equals("passthrough")
  t:expect(proxy:received()[1].status):equals(200)
end)

-- The assertion you cannot get any other way: the dependency is real and answering, and we still
-- know exactly what was said to it.
prova.test("observe: assert on real traffic to a real dependency", function(t)
  local real = http.mock(t)
  real:on{ path_matches = "^/v1/orders" }:reply{ status = 201, json = { id = "o-1" } }

  local proxy = http.mock(t, { passthrough = real.url })

  http.post(proxy.url .. "/v1/orders", {
    json = { sku = "A1" },
    headers = { ["X-Idempotency-Key"] = "k-42" },
  })

  local calls = proxy:received{ method = "POST", path = "/v1/orders" }
  t:expect(calls):has_length(1)
  t:expect(calls[1].headers["x-idempotency-key"]):equals("k-42")
  t:expect(calls[1].json.sku):equals("A1")
end)

prova.test("a stub wins over passthrough — partial mocking", function(t)
  local real = http.mock(t)
  real:on{ path = "/a" }:reply{ status = 200, body = "real-a" }
  real:on{ path = "/b" }:reply{ status = 200, body = "real-b" }

  local proxy = http.mock(t, { passthrough = real.url })
  proxy:on{ path = "/a" }:reply{ status = 200, body = "stubbed-a" }

  t:expect(http.get(proxy.url .. "/a").body):equals("stubbed-a")
  t:expect(http.get(proxy.url .. "/b").body):equals("real-b")
  t:expect(real:received()):has_length(1) -- /a never reached the real service
  t:expect(proxy:received()[1].source):equals("stub")
end)

-- THE drift proof, and the reason observe/replay earns its place over stubbing: the same assertions
-- pass against the real service and against the recording of it. Prove the contract where the
-- service exists; run hermetically where it doesn't.
prova.test("record, then replay with the dependency gone", function(t)
  local cassette = t:tempdir() .. "/pricing.json"

  local real = http.mock(t)
  real:on{ path = "/v1/price/A1" }:reply{ status = 200, json = { cents = 999 } }
  local rec = http.mock(t, { passthrough = real.url, record = cassette })
  t:expect(http.get(rec.url .. "/v1/price/A1"):json().cents):equals(999)
  rec:stop()  -- writes the cassette
  real:stop() -- the dependency does not exist from here on

  local replay = http.mock(t, { replay = cassette })
  local res = http.get(replay.url .. "/v1/price/A1")
  t:expect(res.status):equals(200)
  t:expect(res:json().cents):equals(999) -- identical assertion, no dependency
  t:expect(replay:received()[1].source):equals("replay")
end)

-- Recording real traffic writes real traffic to a file someone will commit. A cassette carrying a
-- live bearer token is a security incident, so redaction is a default, not an option.
prova.test("a cassette redacts credentials by default", function(t)
  local cassette = t:tempdir() .. "/auth.json"

  local real = http.mock(t)
  real:on{ path = "/me" }:reply{ status = 200, json = { ok = true } }
  local rec = http.mock(t, { passthrough = real.url, record = cassette })
  http.get(rec.url .. "/me", { headers = { Authorization = "Bearer sekrit-token" } })
  rec:stop()

  local text = fs.read(cassette)
  t:expect(text):never():contains("sekrit-token")
  t:expect(text):contains("REDACTED")
  -- The journal is in-memory and is NOT redacted: that is where you assert auth was sent.
  t:expect(rec:received()[1].headers["authorization"]):contains("sekrit-token")
end)

prova.test("redact takes extra header names", function(t)
  local cassette = t:tempdir() .. "/extra.json"

  local real = http.mock(t)
  real:on{ path = "/x" }:reply{ status = 200 }
  local rec = http.mock(t, { passthrough = real.url, record = cassette, redact = { "X-Tenant" } })
  http.get(rec.url .. "/x", { headers = { ["X-Tenant"] = "acme-corp" } })
  rec:stop()

  t:expect(fs.read(cassette)):never():contains("acme-corp")
end)

-- Strictness is the feature. A replay that invents an answer for a call it never recorded would let
-- the SUT change behavior without the suite noticing, which is the exact failure cassettes exist to
-- catch.
prova.test("replay does not invent an answer it never recorded", function(t)
  local cassette = t:tempdir() .. "/thin.json"

  local real = http.mock(t)
  real:on{ path = "/known" }:reply{ status = 200, body = "yes" }
  local rec = http.mock(t, { passthrough = real.url, record = cassette })
  http.get(rec.url .. "/known")
  rec:stop()

  local replay = http.mock(t, { replay = cassette })
  t:expect(http.get(replay.url .. "/known").body):equals("yes")
  t:expect(http.get(replay.url .. "/never-recorded").status):equals(404)
end)

prova.test("replay distinguishes query strings", function(t)
  local cassette = t:tempdir() .. "/q.json"

  local real = http.mock(t)
  real:on{ path = "/search" }:reply(function(req)
    return { status = 200, body = "hit:" .. req.query.q }
  end)
  local rec = http.mock(t, { passthrough = real.url, record = cassette })
  http.get(rec.url .. "/search?q=widget")
  http.get(rec.url .. "/search?q=gadget")
  rec:stop()

  local replay = http.mock(t, { replay = cassette })
  t:expect(http.get(replay.url .. "/search?q=gadget").body):equals("hit:gadget")
  t:expect(http.get(replay.url .. "/search?q=widget").body):equals("hit:widget")
end)

-- Repeated identical calls replay in the order they were recorded, so a sequence that changes
-- (create → read-back) reproduces rather than collapsing to its first answer.
prova.test("repeated identical calls replay in recorded order", function(t)
  local cassette = t:tempdir() .. "/seq.json"

  local real = http.mock(t)
  local n = 0
  real:on{ path = "/next" }:reply(function()
    n = n + 1
    return { status = 200, body = "call-" .. n }
  end)
  local rec = http.mock(t, { passthrough = real.url, record = cassette })
  http.get(rec.url .. "/next")
  http.get(rec.url .. "/next")
  rec:stop()

  local replay = http.mock(t, { replay = cassette })
  t:expect(http.get(replay.url .. "/next").body):equals("call-1")
  t:expect(http.get(replay.url .. "/next").body):equals("call-2")
end)

prova.test("passthrough and replay are mutually exclusive", function(t)
  local ok = pcall(http.mock, t, { passthrough = "http://x", replay = "/tmp/c.json" })
  t:expect(ok):is_false()
end)

prova.test("record without passthrough is rejected", function(t)
  local ok = pcall(http.mock, t, { record = "/tmp/c.json" })
  t:expect(ok):is_false()
end)

prova.test("a dead upstream surfaces as 502, and is recorded", function(t)
  local dead = http.mock(t)
  local url = dead.url
  dead:stop()

  local proxy = http.mock(t, { passthrough = url })
  local res = http.get(proxy.url .. "/anything")
  t:expect(res.status):equals(502)
  t:expect(proxy:received()[1].source):equals("passthrough")
  t:expect(proxy:received()[1].error):is_truthy()
end)
