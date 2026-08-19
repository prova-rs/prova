--- `stdio.mock` and `stdio.proxy` — the two listen postures, reached by SPAWN instead of dial
--- (docs/plans/stdio-transport.md §4).
---
--- The idea these pin: **a stdio mock IS a socket mock**, and the only new mechanism is an
--- adapter. `mocks-proxies-drivers.md` already defines a transport as something that can "listen,
--- connect-OR-SPAWN"; the listen postures had simply never exercised the spawn half. So the shim
--- on PATH is two lines and carries no behavior — `exec <prova> relay --to unix://…` — while
--- stubs, journal, cassettes and matching stay in-process where the real matcher is.
---
--- That inversion is the load-bearing part. The older shims (`terminal.mock`, `shell.proxy`)
--- render behavior INTO an `sh` script, which is precisely why their matching cannot go past
--- `case` patterns over bytes: `sh` is the matcher. Here `sh` is a pipe.

local function server(t, body)
  local p = t:tempdir() .. "/upstream.sh"
  fs.write(p, "#!/bin/sh\n" .. body)
  shell.run({ "chmod", "+x", p })
  return p
end

-- ── mock: terminate ────────────────────────────────────────────────────────────────────────────

prova.test("a spawned command name is shadowed, and its turns are matched by SHAPE", {
  requires = { "unix" },
  proves = "stdio.mock: the terminate posture, reached by spawn — and shape matching, which an sh shim cannot do",
}, function(t)
  local fake = stdio.mock(t, { as = "fake-mcp", framing = "line", codec = "json" })
  fake:on({ method = "tools/list" }):reply({ id = 1, result = { tools = {} } })

  -- The SUT spawns `fake-mcp` by NAME, exactly as it would the real server.
  local sess = stdio.spawn(t, {
    cmd = { "fake-mcp" }, env = fake.env, framing = "line", codec = "json",
  })
  -- Deliberately NOT the serialization the stub was declared with: different key order, extra
  -- fields. A byte-matching mock would miss this, and missing it is the whole reason the shim
  -- had to stop being the matcher.
  sess:send({ method = "tools/list", jsonrpc = "2.0", id = 1 })
  local reply = sess:recv({ where = { id = 1 }, timeout = "10s" })
  t:expect(type(reply.result.tools)):equals("table")

  local seen = fake:received()
  t:expect(#seen, "the §6 journal recorded the turn"):equals(1)
  t:expect(seen[1].matched):is_true()
  t:expect(seen[1].source):equals("stub")
end)

prova.test("an unstubbed turn is journaled and the session closes LOUD — never a guess", {
  requires = { "unix" },
  proves = "stdio.mock: an unmatched turn is the most interesting thing a mock records",
}, function(t)
  local fake = stdio.mock(t, { as = "picky", framing = "line", codec = "json" })
  fake:on({ method = "known" }):reply({ id = 1, ok = true })

  local sess = stdio.spawn(t, { cmd = { "picky" }, env = fake.env, framing = "line", codec = "json" })
  sess:send({ id = 9, method = "unknown" })
  local ok = pcall(function() return sess:recv({ timeout = "5s" }) end)
  t:expect(ok, "no stub matched, so nothing is invented"):is_false()

  local missed = fake:received({ matched = false })
  t:expect(#missed):equals(1)
  t:expect(missed[1].source):equals("unmatched")
  t:expect(missed[1].data, "the raw turn is kept, so the author sees WHAT arrived"):contains("unknown")
end)

-- ── proxy: interpose ───────────────────────────────────────────────────────────────────────────

prova.test("a session is recorded turn by turn, then replayed with the upstream GONE", {
  requires = { "unix" },
  proves = "stdio.proxy: the cell shell.proxy cannot fill — its turn is a whole invocation, so a conversation collapses into one blob",
}, function(t)
  local dir = t:tempdir()
  local cas = dir .. "/session.json"
  local upstream = server(t, "while IFS= read -r l; do echo '{\"id\":1,\"from\":\"real\"}'; done\n")

  local rec = stdio.proxy(t, {
    as = "svr", upstream = { upstream },
    framing = "line", mode = "record", cassette = cas,
  })
  local s1 = stdio.spawn(t, { cmd = { "svr" }, env = rec.env, framing = "line", codec = "json" })
  s1:send({ id = 1, ask = "hi" })
  t:expect(s1:recv({ where = { id = 1 }, timeout = "10s" }).from):equals("real")

  local rows = rec:transcript()
  t:expect(#rows, "both directions transcribed"):equals(2)
  t:expect(rows[1].dir):equals("up")
  t:expect(rows[2].dir):equals("down")

  rec:close()                       -- the flush point
  t:expect(cas):exists()

  -- Replay: no upstream at all. The credential-free rerun that makes recording worth doing.
  local rep = stdio.proxy(t, { as = "svr", framing = "line", mode = "replay", cassette = cas })
  local s2 = stdio.spawn(t, { cmd = { "svr" }, env = rep.env, framing = "line", codec = "json" })
  s2:send({ id = 1, ask = "hi" })
  t:expect(s2:recv({ where = { id = 1 }, timeout = "10s" }).from,
    "answered from the cassette with nothing real behind it"):equals("real")
end)

prova.test("a cassette carries its turn model, and a foreign one is refused by name", {
  requires = { "unix" },
  proves = "stdio.proxy: `kind` was advertised as a sanity check and nothing checked it",
}, function(t)
  local cas = t:tempdir() .. "/wrong.json"
  -- A `shell` cassette: argv+stdin → stdout+exit, a genuinely different turn model.
  -- `json.array` because an empty Lua table cannot say whether it is a list or a map, and the
  -- cassette schema wants a list — without it serde rejects the shape before the kind is read.
  fs.write(cas, json.encode({ version = 1, kind = "shell", turns = json.array({}) }))

  local ok, e = pcall(function()
    return stdio.proxy(t, { as = "svr", framing = "line", mode = "replay", cassette = cas })
  end)
  t:expect(ok):is_false()
  t:expect(tostring(e)):contains("recorded by `shell`")
  t:expect(tostring(e), "…and what this reader CAN read"):contains("stdio")
end)

-- ── the vocabulary's honest edge ───────────────────────────────────────────────────────────────

prova.test("turn-level faults ride; byte-level ones are refused, naming where they live", {
  requires = { "unix" },
  proves = "stdio.proxy: a fault that reads as configured and injures nothing is worse than none",
}, function(t)
  local upstream = server(t, "while IFS= read -r l; do echo '{\"id\":1}'; done\n")
  local p = stdio.proxy(t, { as = "slow", upstream = { upstream }, framing = "line" })

  p:latency("2s")                                    -- a turn-level verb: rides
  local sess = stdio.spawn(t, { cmd = { "slow" }, env = p.env, framing = "line", codec = "json" })
  sess:send({ id = 1 })
  local ok = pcall(function() return sess:recv({ timeout = "300ms" }) end)
  t:expect(ok, "the injected delay is observable from the client's bound"):is_false()

  for _, verb in ipairs({ "corrupt", "throttle" }) do
    local ok2, e = pcall(function() return p[verb](p) end)
    t:expect(ok2, verb .. " is refused rather than silently accepted"):is_false()
    t:expect(tostring(e), "…pointing at the byte-level wiretap"):contains("socket.proxy")
  end
end)

-- ── the adapter itself ─────────────────────────────────────────────────────────────────────────

prova.test("prova relay is a real verb: it pipes stdio to a socket, and refuses without one", {
  requires = { "unix" },
  proves = "relay: the adapter is public — a shim whose failures surface inside a generated script nobody can read is a private protocol between prova and itself",
}, function(t)
  -- Driven through prova.bin: this is a verb of the binary UNDER TEST, not of the conductor.
  local usage = shell.run({ prova.bin, "relay" }, { merge_stderr = true })
  t:expect(usage.code, "no destination is a usage error, not a default"):equals(2)
  t:expect(usage.stdout):contains("--to")

  -- It carries bytes and nothing else: point one at a socket.mock and the turns arrive framed,
  -- matched, and journaled — with no protocol knowledge anywhere in the pipe.
  local m = socket.mock(t, { framing = "line", codec = "json" })
  m:on({ ping = true }):reply({ pong = true })
  local r = shell.run(
    "echo '{\"ping\":true}' | " .. prova.bin .. " relay --to " .. m.addr,
    { timeout = "10s" })
  t:expect(r.stdout):contains("pong")
  t:expect(#m:received()):equals(1)

  -- And the bound: a stale shim pointing at a socket nobody listens on must fail, never hang.
  local dead = shell.run({ prova.bin, "relay", "--to", "unix://" .. t:tempdir() .. "/nobody.sock" },
    { merge_stderr = true, timeout = "20s" })
  t:expect(dead.code):never():equals(0)
  t:expect(dead.stdout):contains("connect")
end)
