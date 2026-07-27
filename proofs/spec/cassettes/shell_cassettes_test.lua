--- shell.proxy cassettes — record a real CLI once, replay it forever (docs/design/
--- mocks-proxies-drivers.md; the process transport's turn model already exists: argv + stdin →
--- stdout + exit). The highest-leverage cassette for agent workflows: record `gh`/`kubectl`/
--- `terraform` against reality, commit the cassette, and every later run replays without the
--- binary, the network, or the credentials. Decisions these specs pin:
---
---   * `shell.proxy(ctx, { as, upstream?, cassette?, mode? })` — the standard modes. Replay
---     needs no upstream: the shim answers from the recording.
---   * Match key: the argv (exact) + stdin. A replay miss exits non-zero naming the cassette.
---   * The cassette is written at stop/scope exit (the flush point), and lives OUTSIDE the
---     shim's own directory — `stop` removes the shim, never the recording.

prova.test("record mode captures invocations; stop is the flush point",
  { requires = { "unix" }, spec = "tier-a/shell-cassettes: record mode — not built" }, function(t)
  local cas = t:tempdir() .. "/banner.cassette"
  local shim = shell.proxy(t, { as = "banner", upstream = "/bin/echo", cassette = cas, mode = "record" })

  local r = shell.run("banner release v1", { env = shim.env })
  t:expect(r.stdout):contains("release v1")        -- the real binary answered, recorded
  shim:stop()

  t:expect(fs.exists(cas)):is_true()
end)

prova.test("replay answers from the recording — no upstream, no real binary consulted",
  { requires = { "unix" }, spec = "tier-a/shell-cassettes: replay mode — not built" }, function(t)
  local cas = t:tempdir() .. "/replay.cassette"
  local rec = shell.proxy(t, { as = "banner", upstream = "/bin/echo", cassette = cas, mode = "record" })
  shell.run("banner release v1", { env = rec.env })
  rec:stop()

  local rep = shell.proxy(t, { as = "banner", cassette = cas, mode = "replay" })   -- no upstream
  local r = shell.run("banner release v1", { env = rep.env })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("release v1")        -- the recording answered
end)

prova.test("a replay miss is loud — an unrecorded invocation exits non-zero naming the cassette",
  { requires = { "unix" }, spec = "tier-a/shell-cassettes: loud replay miss — not built" }, function(t)
  local cas = t:tempdir() .. "/miss.cassette"
  local rec = shell.proxy(t, { as = "banner", upstream = "/bin/echo", cassette = cas, mode = "record" })
  shell.run("banner recorded", { env = rec.env })
  rec:stop()

  local rep = shell.proxy(t, { as = "banner", cassette = cas, mode = "replay" })
  -- The recorded invocation answers — which is what makes the miss below a MISS, not a shim
  -- that never replays anything (the assertion that keeps this spec red until cassettes exist).
  t:expect(shell.run("banner recorded", { env = rep.env }).stdout):contains("recorded")

  local boom = shell.run("banner never-recorded", { env = rep.env })
  t:expect(boom.code):never():equals(0)
  t:expect(rep:received{ matched = false }):has_length(1)   -- §6: the miss is journaled too
end)
