--- Cassettes — record/replay as a kernel facility (docs/design/mocks-proxies-drivers.md).
--- A cassette is a RECORDING, not a hand-authored script: the transcript a proxy captures in
--- record mode and replays later. "A Proxy in record mode manufactures a Mock" — prove against
--- reality once, then pin it deterministically forever. It is a Mock you did not have to
--- write, and it is human-editable after capture.
---
--- The kernel owns what is invariant across transports: the MODES (`record` | `replay` |
--- `auto` | `passthrough`), the flush point (cassette written when the proxy closes — scope
--- exit or explicit `:close()`), REDACTION at record time (or replays leak secrets and
--- diff-thrash), and the matching contract (an inbound turn selects a recorded response by the
--- transport's match key; a miss in replay mode is LOUD). Each transport contributes only:
---
---   turn model  — http/grpc: request→response pairs (VCR-shaped) · shell shim: argv+stdin→
---                 stdout+exit · socket: framed turns · terminal: timed frames (asciinema-shaped)
---   match key   — http: method+path(+body) · grpc: full method+request · socket: the framed turn
---
--- Honest limitation, by design: VCR semantics hold on request/response transports; full-duplex
--- transports (raw socket, websocket, terminal) replay as a SCRIPTED CONVERSATION — ordered,
--- expectation-driven, timing-annotated. Same file format, different replay discipline.
---
--- http.proxy is the first specialization and what these specs drive.

local function upstream(t)
  local m = http.mock(t)
  m:on{ method = "GET", path = "/greet" }:reply{ status = 200, json = { msg = "hi" } }
  return m
end

prova.test("record mode captures traffic and flushes the cassette on close",
  { proves = "tier-a/cassettes: record captures while traffic flows; close is the flush point" }, function(t)
  local up = upstream(t)
  local cas = t:tempdir() .. "/greet.cassette"

  local p = http.proxy(t, { upstream = up.url, cassette = cas, mode = "record" })
  t:expect(http.get(p.url .. "/greet").status):equals(200)   -- traffic flows while recording
  p:close()                                                  -- the flush point

  t:expect(fs.exists(cas)):is_true()
end)

prova.test("a proxy in record mode manufactures a mock — replay works with the upstream GONE",
  { proves = "tier-a/cassettes: a proxy in record mode manufactures a mock — reality pinned forever" }, function(t)
  local up = upstream(t)
  local cas = t:tempdir() .. "/greet.cassette"

  local rec = http.proxy(t, { upstream = up.url, cassette = cas, mode = "record" })
  t:expect(http.get(rec.url .. "/greet"):json().msg):equals("hi")
  rec:close()
  up:close()                                     -- reality leaves the building

  local rep = http.proxy(t, { cassette = cas, mode = "replay" })   -- no upstream needed
  local r = http.get(rep.url .. "/greet")
  t:expect(r.status):equals(200)
  t:expect(r:json().msg):equals("hi")            -- pinned deterministically forever
end)

prova.test("auto mode: record when the cassette is absent, replay when it is present",
  { proves = "tier-a/cassettes: auto = record when absent, replay when present" }, function(t)
  local up = upstream(t)
  local cas = t:tempdir() .. "/auto.cassette"

  local first = http.proxy(t, { upstream = up.url, cassette = cas, mode = "auto" })
  t:expect(http.get(first.url .. "/greet").status):equals(200)
  first:close()
  t:expect(fs.exists(cas)):is_true()             -- absent → recorded

  up:close()
  local second = http.proxy(t, { upstream = up.url, cassette = cas, mode = "auto" })
  t:expect(http.get(second.url .. "/greet"):json().msg):equals("hi")   -- present → replayed
end)

prova.test("a replay miss is loud — an unrecorded request is an error, never a guess",
  { proves = "tier-a/cassettes: a replay miss is a 5xx naming the cassette, never a guess" }, function(t)
  local up = upstream(t)
  local cas = t:tempdir() .. "/miss.cassette"

  local rec = http.proxy(t, { upstream = up.url, cassette = cas, mode = "record" })
  http.get(rec.url .. "/greet")
  rec:close()

  local rep = http.proxy(t, { cassette = cas, mode = "replay" })
  local r = http.get(rep.url .. "/never-recorded")
  t:expect(r.status):gte(500)                    -- a 5xx naming the cassette + the missed key
  t:expect(r.body):contains("cassette")
end)

prova.test("redaction happens at record time — the secret never touches disk",
  { proves = "tier-a/cassettes: redaction happens at record time — the secret never touches disk" }, function(t)
  local up = upstream(t)
  local cas = t:tempdir() .. "/redacted.cassette"

  local p = http.proxy(t, {
    upstream = up.url, cassette = cas, mode = "record",
    redact = { headers = { "authorization" } },
  })
  http.get(p.url .. "/greet", { headers = { authorization = "Bearer hunter2" } })
  p:close()

  t:expect(fs.exists(cas)):is_true()
  t:expect(fs.read(cas)):never():contains("hunter2")
end)

prova.test("passthrough mode ignores the cassette entirely — flow, record nothing",
  { proves = "tier-a/cassettes: passthrough ignores the cassette — flow, record nothing" }, function(t)
  local up = upstream(t)
  local cas = t:tempdir() .. "/untouched.cassette"

  local p = http.proxy(t, { upstream = up.url, cassette = cas, mode = "passthrough" })
  t:expect(http.get(p.url .. "/greet").status):equals(200)
  p:close()

  t:expect(fs.exists(cas)):is_false()
end)
