# doubles — mocks and containers: standing in for what the SUT needs

A proof is black-box: green must mean the SYSTEM works. So the default stand-in for a
dependency is the REAL dependency, ephemeral: a **container double** — `postgres.container(ctx)`
gives the app a real engine that exists only for the proof's scope. Reach for a **mock** only
where it earns its place:

| Situation | Reach for |
|---|---|
| The app needs postgres/redis/kafka/... | The real thing: `X.container(ctx)` (a plugin), or `docker.run{...}` + `prova.retry` |
| The dependency can't run here (3rd-party SaaS, paid API, flaky upstream) | `http.mock` / `grpc.mock` — virtualize it |
| The assertion IS the interaction ("did the app call billing exactly once, in order?") | a mock's journal — only a mock can see it |
| Fault injection (500s, timeouts, malformed replies) | a mock stub returning the fault |
| An in-process, function-shaped seam (a plugin effector, an injected Lua dependency) | `require("prova.double")` |

Mocking what you could run for real trades away exactly the evidence a proof exists to produce.

## The shipped surface

```lua
local m = http.mock(t)                                   -- a REAL server, this process, ctx-scoped
m:on{ method = "GET", path = "/health" }:reply{ status = 200, json = { ok = true } }
m:on{ method = "POST", route = "/echo/:word" }:reply(function(req)  -- a handler is real Lua
  return { status = 200, json = { echoed = req.params.word } }
end)
-- point the SUT at m.url, drive it, then assert on the JOURNAL (an unmatched request is
-- usually the most interesting thing a mock can tell you):
t:expect(m:received{ path = "/health" }):has_length(1)
```

- `grpc.mock(ctx)` — same grammar over gRPC; serves reflection, so `grpc.client` just connects.
- **Proxy dial**: `http.mock(ctx, { target = real_url })` — unmatched requests pass through to
  the real service and are LOGGED; stubs still win (partial mocking). Record real traffic,
  replay hermetically.
- **In-network vantage**: a containerized SUT reaches a host-bound mock via `m.network` inside a
  topology (`ctx.network`) — wiring a container to `127.0.0.1` is the classic mistake.
- `require("prova.double")` — transport-free callable double: `d:on{...}:reply(...)`,
  `d:received{...}`, mock/proxy/spy by which knobs you set.

## The grammar behind all of this

Every resource namespace has the same facets: `X.client` attaches to something running ·
`X.container` provisions the real thing · `X.wait_for` probes readiness · `X.mock` provisions a
fake one. Plugins declared in this package add their facets to the vocabulary:

{{plugins}}

Not yet shipped (do not reach for them): standalone interposing proxies as a facet of their own,
`net.mock` (raw TCP/unix), `graphql.mock`. `prova.help("mock")` lists what exists in this build.

Go deeper: `prova learn project` (where plugins are declared) · `prova learn pdd` (why real-first).
