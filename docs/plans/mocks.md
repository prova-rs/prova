# Plan: mocks — the dependency you can't run, and the interactions you can't see

Design refs: `docs/design/foundations.md:154` (`mock` named as a first-party seam — "a WireMock-style
stub server fixture for external dependencies"), `docs/design/api.md:145-153` (an `http.serve_mock`
sketch that exists nowhere else in the repo), `docs/design/namespacing.md:13-22` (the facets),
`docs/design/topologies.md` (the resource grammar and the verbs), roadmap Phase 2 item 5
("Full cross-service acceptance suite — the proof of the whole thesis", `north-star-roadmap.md:205`).

## The stance, first — because it decides the scope

Prova's differentiator is that **it runs the real thing**. `container_app.lua` proves a containerized
SUT talking to a real Postgres by DNS alias on a topology network. For a service you own, "mock it
out" is the *worse* answer and we should not sell it. Mocking earns its place on exactly four
boundaries, and the plan is scoped to those:

1. **The dependency you cannot run** — a partner API, a SaaS, a service that isn't in this repo.
2. **The behavior the real thing won't produce on demand** — 5xx, timeouts, malformed bodies, rate
   limits, a connection dropped mid-response.
3. **The interaction itself** — *did* we call it, exactly once, with the right idempotency key? A
   real dependency answers "did it work"; it does not answer "what did we say". **This is the piece
   that has no substitute today, and it does not require mocking at all** (see Passthrough).
4. **Speed/hermeticity in CI** — replay a recorded cassette instead of reaching the network.

The counter-pressure is real and stays in force — `proof-driven-development.md:87-89`: a proof is
black-box *"so a green result means the system works, not that an internal mock agreed with another
internal mock."* A mock at the **downstream boundary** does not violate this: the SUT is still driven
through its real API. A mock *of the SUT* would, and is a non-goal.

## Target

The whole design in one screen. A mock **is a resource** — its `url` wires into the SUT exactly like
a database's, so the topology machinery (auto-join, vantages, port modes, `ctx:manage`, `up`/`watch`)
applies with no new concepts.

```lua
local pricing = prova.fixture("pricing", Scope.File, function(ctx)
  local m = http.mock(ctx)                                   -- resource: { url, network, … }

  m:on{ method = "GET", path = "/v1/price/A1" }
   :reply{ status = 200, json = { sku = "A1", cents = 999 } }

  m:on{ method = "GET", path_matches = "^/v1/price/" }       -- Lua handler: the primitive
   :reply(function(req) return { status = 404, json = { error = "unknown sku " .. req.path } } end)

  return m
end)

local app = prova.fixture("app", Scope.File, function(ctx)
  local m = ctx:use(pricing)
  return prova.containerized{
    name = "app",
    build = ".",
    port = 8080,
    env = function(o) return { PRICING_URL = o.pricing_url } end,
    url = function(hp) return "http://127.0.0.1:" .. hp end,
  }.container(ctx, { pricing_url = m.network.url })          -- NETWORK vantage, as with any resource
end)

prova.test("checkout prices via the pricing service", function(t)
  local m, a = t:use(pricing), t:use(app)

  local res = http.post(a.url .. "/checkout", { json = { sku = "A1", qty = 2 } })
  t:expect(res.status):equals(200)
  t:expect(res:json().total_cents):equals(1998)

  -- Assertions on the mock are just the existing matchers over recorded data. No new vocabulary.
  local calls = m:received{ path = "/v1/price/A1" }
  t:expect(calls):has_length(1)
  t:expect(calls[1].headers["x-idempotency-key"]):is_truthy()
  t:expect(m:received()):matches_snapshot("pricing-conversation")
end)
```

## The core design — four decisions, each forced by something already in the tree

### 1. `mock` is a **facet**, not a namespace

`namespacing.md:6-9` allows one namespace per *technology you speak*. "Mock" is not a technology, so
`foundations.md:154`'s `mock` module contradicts the grammar we settled later. The facet list grows
from three to four — `client` (attach to a real one) · `container` (provision a real one) ·
`wait_for` (probe one) · **`mock` (provision a fake one)** — and `engine.rs:3867`'s lint array grows
to match. Facets are already optional per namespace (`sqlite` has no `container`), so `mock` living
only on protocol namespaces (`http`, `grpc`, `graphql`, `net`) is consistent, not an exception.

The ecosystem payoff is the actual argument: a third-party `stripe` plugin ships `stripe.mock(ctx)`
— canned Stripe semantics over the core primitive — and inherits the entire grammar for free. That
is `prova.containerized`-composes-`docker.run` one layer up, and it is the shape service
virtualization has to take here.

### 2. It is a **core primitive**, not a plugin — and `foundations.md:154` is wrong about this

`ecosystem.md:310-315` is unambiguous: native code is always first-party and bundled; *"a plugin
author writes **Lua + Docker**, never native code."* You cannot write a server in Lua. So a mock
server is either a Docker image (WireMock-in-a-container: an image to pull, an admin API to round-
trip, ~1s of startup, and a JSON DSL between you and your assertions) or it is Rust in the binary.
It is Rust in the binary — same tier as the `http` and `grpc` clients.

`foundations.md:156-158` justifies plugin-ness with *"so the agnostic core never grows a Docker
dependency"* — and a mock server needs no Docker at all. The stated reason does not apply to this
module. **Fold the correction into `foundations.md` when this lands**; the line predates both the
native-client extraction and the no-third-party-native rule.

### 3. Lua handlers work — and that is the differentiator

This was the decision most likely to go the wrong way, so it is recorded with its evidence.

The instinct from a blocking-client design is "the Lua state is busy inside `http.post`, so a server
task can never call a handler; therefore stubs must be declarative data, therefore we need a
templating mini-language for dynamic responses." **That is false here.** `engine.rs:3` — *"Async is
foundational (bodies driven via `call_async`; many run concurrently on one Lua state)"* — and the
whole IO surface is `create_async_function` (`shell.run` `modules.rs:467`, `http.*` `:923`/`:1022`,
`container:run` `:1168`). Test bodies are already concurrent coroutines on one Lua state. A mock
server spawned onto the same current-thread runtime is driven whenever the SUT-driving call awaits,
and its handler is one more coroutine on the state the engine already multiplexes.

So we get what WireMock invented Handlebars to fake: **the handler is real Lua**. That kills the
mini-language before it is born, which is the same instinct that kept the assertion surface from
being `expect(x, "equals", y)`.

Declarative `:reply{…}` stays as the terse form — it is what `prova up` can serve with no test in
scope, and what a cassette round-trips to. Per `topologies.md:162-164`, *the convenience never
removes the primitive*: `:reply{table}` is the convenience, `:reply(function)` is the primitive.

**The sharp edge, named:** a handler runs while the test coroutine is suspended, so it is a new form
of concurrent Lua — precisely the shared-mutable-state a `group` is designed never to grant
(`api.md:230-236`). Two rules keep it from becoming the footgun: a handler receives the request and
returns a response, and **is not given `t`** (no assertions from inside a handler — assert on the
journal afterward, where failures have a test to attach to); and a handler that raises records the
error into the journal, answers 500, and **fails the owning test at scope end** rather than
vanishing into a server task.

### 4. Passthrough is the same object, one option — and it's the most valuable mode

A proxy is a mock whose unmatched requests are forwarded rather than 404'd. That is one option, not
a second concept, and the journal/stubs/grammar are identical:

| Mode | Written as | Dependency | Answers |
|---|---|---|---|
| stub | `http.mock(ctx)` | none | the boundary you can't run; edge cases |
| **observe** | `http.mock(ctx, { passthrough = real.url })` | **real** | *"what did we say?"* — with zero behavior change |
| record | `{ passthrough = real.url, record = "…" }` | real | produce a cassette |
| replay | `{ replay = "…" }` | none | hermetic, fast CI |
| fault | `:reply{ delay = "5s" }` / `{ abort = "reset" }` | either | the behavior the real thing won't produce |

**Observe mode is the answer to the drift objection** that sinks most mocking: run the suite against
the real service to prove the contract, replay the cassette in CI. It is also the only mode that is
purely additive to the black-box thesis — the dependency is real, the traffic is real, and we only
watched. It is worth building even if nobody ever writes a stub.

### 5. Two vantages, because owning the network buys transparent interposition

Everything above wires a mock in by **rewriting a URL** into the SUT's env — which silently assumes
the SUT *has* a `PRICING_URL` injection point. Plenty don't: a third-party image, a hard-coded
discovery name, a config baked at build time. And a test-only env var means the thing under test is
not quite the thing that ships.

On a topology network there is a strictly better move: **take over the DNS alias.** The real service
joins as `pricing-real`; the interposer joins as `pricing`. The SUT's *unmodified production config*
still says `pricing:8080`, and we are in the path. Nothing about the SUT is test-shaped.

The property that makes this work rather than merely sound clever: **each topology already gets its
own Docker network** (`engine.rs:841-855`), so the alias `pricing` is namespaced per-topology. Ten
suites in parallel each have their own `pricing` and cannot collide — which the host-port equivalent
(`--add-host=pricing:host-gateway` + a mock bound to `:8080`) can never offer, because host ports are
global. That is the whole argument for paying an image cost here.

So the mock/proxy engine gets the same two vantages the SUT already has (`topologies.md:162-164` —
*"a host-run SUT or a containerized one. Both coexist; the convenience never removes the primitive"*):

| Vantage | Written as | Reached by | Cost | Buys |
|---|---|---|---|---|
| **host** (default) | `http.mock(ctx)` | `host.docker.internal:<port>` | none — in-process | zero startup; needs a reconfigurable SUT |
| **network** | `http.mock(ctx, { alias = "pricing" })` | DNS alias on the topology network | one tiny image pull | transparent interposition; SUT unmodified |

**The in-network component is a dumb TCP forwarder, not the engine — and that is load-bearing.** It
accepts on the alias's port and forwards to `host.docker.internal:<host_engine_port>`. Nothing else.
The matching, journal, stubs, and **Lua handlers stay host-side where the Lua state is**, so there is
one engine and one code path, and handlers work identically in both vantages. Putting the engine in
the container would fork the implementation and strand handlers behind an admin API — i.e. it would
reinvent WireMock's architecture and inherit its reason for needing a templating language.

**The honest costs**, named so they don't surprise us in week two:
- **Passthrough is a triangle** — SUT → shim → host engine → real service via its *published* port.
  Traffic leaves the network and comes back. Latency is localhost-ish and fine; the real constraint is
  that a legitimately network-only resource must be auto-published for the host engine to reach it.
  Prova controls port publishing, so it can do this — but it is a behavior change, not a no-op.
- **The real service sees the host's source IP**, not the SUT's. Fine for nearly everything; not fine
  for IP allowlisting or per-client rate limits. Document it rather than discover it.
- The shim image is prova-published and version-pinned to the release (`ghcr.io/prova-rs/shim:<v>`),
  built from one static binary. It is a pull, gated by `requires = { "docker" }` which the network
  vantage already implies.

## Plumbing

1. **Runtime — settled in Phase A, and it cost one function.** Server tasks run on the existing
   per-run current-thread runtime; no `rt-multi-thread`, no dedicated thread. The wrinkle the plan
   originally missed: test bodies are futures in a `FuturesUnordered`, never `tokio::spawn`ed, so
   nothing in the engine had ever needed to be `Send` — but a mock server *outlives the call that
   created it* and holds Lua handles, and `tokio::spawn`'s `Send` bound is on `spawn`, not on the
   runtime flavor, so `rt-multi-thread` would not have helped. The mechanism is `spawn_local`, which
   needs a `LocalSet` to be the thing being polled: `engine::block_on_local` wraps every `block_on` in
   one (all 7 sites, so no execution path is the odd one out). Mutation-checked — revert one site to a
   bare `block_on` and the proof dies with *"`spawn_local` called from outside of a `task::LocalSet`"*.
2. **Deps — `axum` was the wrong call; it cannot express this.** A stub's reply may be a Lua function,
   mlua handles are `!Send`, so the service future that calls one is `!Send` — and axum bounds its
   handlers `Send` at the type level. No wrapping recovers it. **Raw `hyper::server::conn::http1`**
   puts no `Send` bound on the service or its future, so it takes a `!Send` handler and is driven by
   `spawn_local` next to the test coroutine it answers. It is also *free*: `hyper`, `hyper-util`,
   `http-body-util`, `bytes`, and `form_urlencoded` are all already compiled into the binary via
   bollard/tonic/reqwest, so Phase A added five declarations and **zero new compilation**. The
   constraint picked the dep, and it picked a cheaper one. (`tonic` still gains `server` for Phase B.)
   Feature: `mock`, default-on, implying `http`.
3. **The vantage inversion — the one genuinely new integration point.** Every existing resource is a
   container the host reaches; a mock is a **host process a container must reach**. `network.url`
   cannot be an alias rewrite (`modules.rs:329-335`). It is `http://host.docker.internal:<port>`, and
   `prova.containerized` must add `--add-host=host.docker.internal:host-gateway` when a topology holds
   a host-bound mock (Docker 20.10+, Linux included). Get this wrong and it fails the way Proof 4 was
   mutation-checked against: silently talking to the wrong thing.
4. **Ports.** Random by default (parallel-safe, like containers); honor the existing `prova.ports`
   signal (`modules.rs:250-260`) so `up --fixed` puts the mock on a predictable port you can `curl`.
5. **Journal.** `Arc<Mutex<Vec<Recorded>>>`, exposed as userdata. `m:received(filter?)` returns plain
   Lua tables — assertions are the existing matchers, and no `verify(count, pattern)` DSL is added.
   The handle implements the snapshot protocol (`snapshots.md:60-65`), so a recorded conversation is a
   snapshot subject, and it feeds the "rich attachments / HTTP exchange" seam (`foundations.md:196`).
6. **gRPC needs a schema — the client's reflection trick does not invert.** `grpc.client` learns the
   schema *from* the server; a mock *is* the server. Descriptors come from one of three sources, in
   build order: a `FileDescriptorSet` (buf output, zero new deps) · a `.proto` via `protox` (pure
   Rust, no `protoc`) · **harvested from the real service over reflection** (`grpc.mock(ctx, { from =
   real.url })` — the mock's schema is by construction the real schema; the drift answer again).
   `DynCodec` (`modules.rs:2437-2480`) already encodes and decodes `DynamicMessage` and works
   unchanged in the server direction. **The mock serves reflection itself**, so the existing
   `grpc.client` drives it with no special case — which is also how we prove it is faithful.

## Build sequence

- **Phase A — `http.mock`. DONE (2026-07-16).** Bind, declarative stubs, **Lua handlers**, journal,
  resource shape (`url`/`host`/`port`), `ctx:manage` teardown. 14 proofs in `testdata/http_mock.lua`
  / `tests/http_mock.rs`, green, no docker needed, ~24ms.

  **The runtime assumption is real, and it is proved rather than argued.** A handler answers while
  the driving coroutine is suspended inside `http.post`, computes its response *from the request*,
  and mutates a test-local upvalue the test then reads back — none of which can pass by accident.
  Mutation-checked twice: strip the `LocalSet` → `spawn_local` panics; the re-entrancy proof (a
  handler calling `m:received()` / `m:on{}` mid-request) would panic on a leaked `RefCell` borrow
  rather than fail politely, so the "borrow released before awaiting into Lua" claim is a test, not a
  comment. **Decision 3 stands: no response-templating language, now or later.**

  Deferred out of A, deliberately: the **host-gateway vantage** (`m.network.url` +
  `--add-host=host.docker.internal:host-gateway`) went with Phase C, where the alias work makes both
  vantages one coherent piece rather than two half-answers to "how does a container reach this". A
  host-run SUT needs neither today.

  Still open from A, and honest about it: a **handler that raises answers 500 and lands `error` in the
  journal, but does not yet fail the owning test at scope end** (decision 3's second rule). It is
  observable — `m:received()[1].error` is asserted in the proof — but a suite that never looks would
  read a broken handler as a legitimate dependency failure. Needs engine integration (a finding
  attached to the scope); tracked here rather than silently dropped.
- **Phase A′ — the aspirational examples.** `examples/aspirational/{ordering,dependent_flows,http_service}.lua`
  are non-runnable for exactly one reason — *"they reference a live service (`http://localhost:8080`
  with no server behind it)"* (`examples/aspirational/README.md:1-12`). `http.mock` is now the server
  behind it. Cheap, and it converts three design sketches into three running proofs.
- **Phase B — `grpc.mock`.** Descriptors (set → `.proto` → harvest), reflection server, `DynCodec`
  reversed. This is the user-facing ask and the Phase-2-item-5 unblocker.
- **Phase C — passthrough / record / replay, and the network vantage.** The observe dial, plus the
  shim + alias interposition (decision 5). Cassette format is a snapshot in all but name; reuse the
  machinery rather than inventing a file format. The two halves ship together because interposing on
  an alias with nothing to forward *to* is not a feature — passthrough is what makes the alias
  takeover worth doing.
- **Phase D — `net.mock`.** One byte-stream namespace, transport as an option (`{ listen = "tcp" }` /
  `{ listen = "unix", path = … }`) — TCP and Unix-stream are the same API at a different address, so
  two thin namespaces would be over-namespacing. **Unix sockets get their vantage for free where it
  matters**: the container reaches it by **bind-mounting the socket path**, not by DNS or gateway —
  cleaner than the TCP case. This is also where fault injection is sharpest (accept-and-hang,
  mid-stream RST, slow-drip) — you cannot make a real Postgres drop a connection mid-query, but a
  byte-level proxy in front of it can.
- **Phase E — `graphql.mock`.** Composes `http.mock`; a recipe, not an engine. Cheap once A lands.

## Verify

Per `container-app-handoff.md:51-62` — proof first as `testdata/<name>.lua`, red for the right
reason, green, **then mutation-check the green**, then `tests/<name>.rs` for CI.

- **A: done** — see the build sequence. Every probe is a single attempt with no `prova.retry` (the
  `docker_readiness.lua` bar), which `http.mock` earns by binding the listener *synchronously* before
  returning: in-process means no daemon in the middle to lie about readiness.
- **A, with a containerized SUT:** deferred to C with the vantage work — SUT reads a stubbed
  dependency through `m.network.url`; journal records it. Mutation-check: swap `m.network.url` →
  `m.url` and confirm red (`127.0.0.1` inside a container is that container — the trap Proof 4 caught).
- **B:** `grpc.client(mock.url)` — the *existing, unmodified* client — drives `grpc.mock` via
  reflection. If the real client can't tell it from a server, it is a server.
- **C:** same suite green in `passthrough` and in `replay` off the cassette it recorded. That
  equivalence *is* the drift proof.
- **C (interposition):** the bar is a **SUT with no injection point** — its dependency URL baked to
  `pricing:8080` at build time, no env override compiled in. It must pass unmodified with the shim
  holding the alias and the real service at `pricing-real`, and the journal must show the calls.
  Mutation-check: drop the alias takeover and confirm the SUT reaches the real service directly and
  the journal goes empty — otherwise the proof passes on the env-rewrite path and proves nothing.
  Second bar: two instances of the suite in parallel, each with its own `pricing`, both green — the
  per-topology-network scoping is the entire justification for the image and must be proved, not
  assumed.
- **D:** a `unix.mock` bind-mounted into a container; an injected mid-stream reset surfaces as the
  SUT's real error path.
- Throughout: `cargo test` + `clippy` + LuaLS, `library/modules.lua` stubs updated in the same commit.

## Non-goals (explicit)

- **Mocking the SUT.** The SUT is always real. Anything else voids `proof-driven-development.md:87-89`.
- **A request-matching DSL.** Declarative fields for the common case; Lua for everything else. If a
  match needs logic, it is a function — that is the whole point of shipping a language.
- **A response-templating language.** Handlers are Lua. This is the mini-language we do not build.
- **`verify(count, pattern)` / `assert_called_with` vocabulary.** The mock exposes data; `t:expect`
  already asserts. New matchers only if the journal proves to need one.
- **In-process/unit mocking.** `foundations.md:207-209` — we cannot mock a Java private method and
  should not pretend. Out-of-process boundaries only.
- **A container-based mock *engine* (WireMock image).** Rejected in decision 2, and decision 5 does
  not walk it back: the in-network component is a **dumb TCP forwarder**, and the engine stays
  host-side in every vantage. The distinction is the whole point — put the engine in the container and
  you fork the implementation, strand Lua handlers behind an admin API, and re-derive the need for a
  templating language. Revisit only if the in-process runtime assumption collapses.
- **Contract testing (Pact-style broker, consumer-driven contracts).** Observe+replay covers the 80%
  that matters here. A broker is a product, not a plan item.

## Remaining / open

- **`http.serve_mock` in `api.md:145-153` becomes `http.mock` when this lands** — the autouse example
  is the only place the old name appears anywhere in the repo, so the rename costs one doc edit.
- **`prova up` + mocks is a reveal, not a phase.** Once A lands, `prova up orders` stands up a
  topology whose third-party edges are stubbed and *holds it* — a fake-backed environment a front-end
  dev inhabits. That is `topologies.md`'s "one definition, multiple consumers" extending to the
  boundary, and it wants a paragraph in `topologies.md` rather than work here.
- **Trigger discipline (`north-star-roadmap.md:165-169`).** Phases D and E have no consumer yet.
  D's trigger: a real suite that needs fault injection or a socket daemon. E's trigger: a GraphQL SUT
  with a GraphQL dependency. Do not build them ahead of one.

### The reveal Phase C makes possible — recorded, not scheduled

Once the shim can hold an alias, it can hold *every* alias. A topology-wide `observe` would interpose
a recording shim on every edge and hand back the **whole conversation graph** — every inter-service
call in the topology, recorded and assertable, with nothing instrumented and no SUT modified:

```lua
t:expect(e:conversation()):matches_snapshot("order-placement")   -- speculative
t:expect(e:conversation{ from = "app", to = "pricing" }):has_length(1)
```

That is distributed-tracing-grade visibility from a test runner, and the reason it is even
expressible is that **prova owns the network** — it wrote the topology, so it can sit on every edge.
No existing tool can do this because none of them own both the topology definition and the assertions
over it. It is the sharpest version of "one definition, multiple consumers" (`topologies.md`).

**It is not scheduled, and should not be built until something asks for it.** Every piece it needs
(shim, alias takeover, journal, snapshot protocol) falls out of Phase C, so it stays a cheap additive
move later — exactly the `describe_each` posture (`north-star-roadmap.md:165-169`), and the reason to
write it down now is so Phase C's shim is not designed in a way that forecloses it. **Its trigger:**
a suite where the interesting assertion is about an edge *between two dependencies* rather than an
edge from the SUT — the first time someone asks "did the order service call inventory before it
called billing," this is the answer, and nothing else in the plan is.
