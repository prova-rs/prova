# Topologies — one definition, multiple consumers

Drafted 2026-07-14. The north-star that reframes what Prova *is*. Where [ecosystem.md](ecosystem.md)
covers *wrapping* resources and [architecture.md](architecture.md) covers the test runner, this
records the larger identity those two pillars fuse into — and the design seam that makes it real.

## The identity

Prova is two platforms welded together:

1. **A test runner** — fixtures, the dependency DAG, the scheduler, assertions, reporters, isolation.
2. **A resource-orchestration layer** — provision ephemeral infra, wire it, drive it, tear it down.

The weld is **the grammar** (`{ client, url, container }`, `ctx:manage`, `requires`, `prova.retry`):
every resource — bundled or plugin, native or docker-exec, a database or a whole Kubernetes topology
— presents the same shape, so there is *one* pattern to learn, not N integrations. That is the moat
versus pytest, where resources are bring-your-own and inconsistent.

Seen this way, "testing" is not the whole product — it is **the first consumer of a more general
substrate**: *provision + wire + drive an ephemeral topology*. Asserting over that topology is one
thing you can do with it; **inhabiting** it (standing it up to develop against) is another. Same
substrate, different terminal verb.

## The Holy Grail: `prova up` and `prova` on the same definition

> **One topology definition. Multiple consumers.**

You describe a topology once — resources, wiring, how they're driven — in Lua. Different verbs consume
the *same* definition:

- **`prova`** (the run path) — bring it up, drive it, **assert**, tear down. *(today)*
- **`prova up`** — bring it up, print the endpoints, **hold it running** for you to develop against,
  tear down on signal. *(the reveal)*
- **`prova watch`** — the above plus a live re-apply loop. *(done — see below)*

The point is not "it does both." It is that the **same definition powers your tests and your dev
environment, so they cannot drift.** Today a compose file, a testcontainers setup, k8s manifests, and
test fixtures are four separate descriptions of "the same" environment that silently diverge. Prova
collapses them to one. No existing tool does this, because they are separate tools.

```lua
local env = prova.topology("orders", function(ctx)
  local db  = require("postgres").container(ctx)
  local mq  = require("kafka").container(ctx)
  local app = boot_app(ctx, { db = db.url, kafka = mq.url })   -- wiring via the grammar's `url`
  return { db = db, mq = mq, app = app }
end)

prova.test("an order lands in the DB", function(t)
  local e = t:use(env)                        -- test: instantiate → drive → assert → teardown
  e.app:post("/orders", { sku = "A1" })
  t:expect(e.db.client:query_value("select count(*) from orders")):equals("1")
end)
```

```
prova test           # runs the assertions against `env`
prova up orders      # stands up the SAME `env`, prints endpoints, holds until Ctrl-C
```

## Why it's a layer, not a rewrite

The abstraction that makes "same fixtures for both verbs" work already exists: **scope + `ctx:manage`.**
A fixture that does `ctx:manage(resource)` is already **verb-agnostic** — it declares *"I own this
resource's lifecycle,"* not *"tear it down at test-end."* The *when* of teardown belongs to the
**scope**, and the scope's lifetime is set by the **mode**:

| Mode | Scope lifetime | Terminal action |
|---|---|---|
| `test` | test / file / suite scope | assert, then tear down at scope-end |
| `up` | an **environment scope** held until signal | print endpoints, hold, tear down on Ctrl-C |

Same fixture code; the fixture never knows which verb runs it. That is why this is additive.

## The one hard part: fixtures are lazy

Prova fixtures are **demand-driven** — instantiated on `t:use(...)`. `prova up` has no tests, so nothing
triggers the demand, so nothing provisions. The bridge is an explicit **topology**: a *named*,
verb-agnostic bundle of wired resources that both verbs address. A topology is, in essence, *a fixture
designed to be a whole environment and addressable by name* — `t:use(env)` instantiates it under a
test scope; `prova up orders` instantiates the identical object under a held environment scope.

## Where the grammar pays off again

- **Endpoint reporting** — `prova up` prints each resource's `url`, so you get "postgres →
  `postgres://…:54432`, kafka → `127.0.0.1:…`, app → `http://…`" and connect immediately. The `url`
  field *is* the connect string.
- **Teardown** — the scope machinery already reaps `ctx:manage`d resources; `up` triggers it on signal
  instead of at scope-end.

## Status

- **`prova.topology(name, [scope,] fn)`** — **done.** A named, verb-agnostic fixture (default
  `Scope.File`), registered so verbs can address it by name. In test mode it is used exactly like any
  fixture (`t:use(handle)`).
- **`prova up <name>` (attached)** — **done.** Loads the manifest's files, provisions the named
  topology under a held File scope, prints each resource's `url`, and blocks until **SIGINT or
  SIGTERM**, then runs the existing `ctx:manage` teardown. Verified with a real Postgres container
  (endpoint on a live host port; container reaped on Ctrl-C). A running `up` **self-registers** a
  record under `<home>/running/<name>.json` (pid + endpoints; self-gitignored) and removes it on
  clean teardown.
- **Detached mode** (`prova start` / `prova down` / `prova ps`) — **done**, and exactly the thin
  **supervisor over attached `prova up`** the design predicted: `start` spawns `prova up <name>` in
  its own process group (stdio → `<home>/running/<name>.log`), waits for it to self-register, prints
  the endpoints, and returns leaving it running; `down` reads the record and `SIGTERM`s the pid, so
  the *same* in-process `ctx:manage` teardown runs in the detached child; `ps` lists records (cleaning
  stale ones). **One provisioning path, one teardown path** — no resource-inventory tracking, no
  survive-process-exit container semantics, no second teardown implementation. Verified end-to-end with
  a real Postgres container (survives `start` returning; reaped by `down`) and a no-docker CLI
  integration test proving the detached child runs teardown on `down`.

## Port modes — external reachability (done)

The definition is written once; the **verb** picks the port strategy, so the seam stays clean:

1. **Testing** — random host ports (parallel-safe). `prova`.
2. **Inhabited, random** — `prova up`/`start` provision on random host ports and print each endpoint,
   so many topologies coexist without collisions.
3. **Inhabited, fixed** — `prova up`/`start --fixed` pin each published port to its canonical container
   port, giving a predictable address real external tools connect to, and letting an advertised-listener
   resource (Kafka) compute its listener because the host port is known up front.

Mechanism: `RunConfig::ports: PortMode` (`Auto`/`Fixed`), exposed to Lua as `prova.ports`
(`"auto"`/`"fixed"`). `prova.containerized` upgrades random ports to fixed bindings under `--fixed`,
leaving author-declared `{ container, host }` entries as-is. Verified live: `up --fixed` binds and is
reachable on `5432`/`6379`; default `up` uses random ports. This settles the **external reachability**
question for the common case; the Kafka advertised-listener recipe is a plugin-side follow-up that
now has the core signal it needs.

## `prova watch` — the inhabited dev loop (done)

`prova watch <name>` stands the topology up, prints its endpoints, and re-provisions whenever the
definition files change — the Tilt-ish live loop, over the *same* definition the tests use. Each pass
builds a fresh Lua state so edits take effect; a failed edit is reported and the loop waits for the fix
rather than exiting. Dependency-free mtime polling with a short settle (one save → one re-apply);
attached-only, pair with `--fixed` for endpoints stable across re-applies. `up` and `watch` share the
provisioning path (`load_topology`/`provision`), so there is one definition-to-resources route, not two.

## The containerized SUT — `build` instead of `image` (done)

The payoff of the networked-topology arc: the **system under test runs in a container too**, wired to
topology resources over the network. The host then needs **nothing but Docker** — no SDK, no JVM, no
uv — and the artifact under test is the project's **real production image**, not a host-built
approximation.

The shape this landed in is the one that adds no concepts: a SUT **is** a resource, one whose image is
*built* rather than *pulled*. `prova.containerized` takes `build` where a published resource takes
`image`:

```lua
local app = prova.containerized{
  name = "app",
  build = { context = ".", dockerfile = ".platform/docker/local/Dockerfile" },
  port = 8080,
  env = function(opts) return { DATABASE_URL = opts.database_url } end,
  url = function(hp) return "http://127.0.0.1:" .. hp end,
}.container(ctx, { database_url = db.network.url })   -- wired via the DB's NETWORK vantage
```

Everything downstream is inherited unchanged — the topology auto-join, the network vantage, readiness,
teardown, port modes — which is precisely why this is a ~15-line delta rather than a subsystem. The
author still chooses per fixture: a host-run SUT (`shell.spawn`, resource **host** urls) or a
containerized one (`build`, resource **network** urls). Both coexist; the convenience never removes
the primitive.

Underneath sits the primitive it needed: **`docker.build{ context, dockerfile?, tag?, buildargs?,
target?, pull?, nocache? }`** → an image ref for `docker.run`. It shells out to the `docker` CLI (as
`create_managed_network` already does, and at no cost in requirements — the `docker` capability gate
already probes `docker info` through that same CLI). That is what buys **BuildKit cache mounts**
(`RUN --mount=type=cache,target=/root/.nuget` — the answer to "naive builds are glacial") and
**`.dockerignore`** honored client-side, both of which driving the HTTP build endpoint would have cost
us. The default image tag is derived from the context path, so it is *stable across runs*: rebuilds
replace the tag and hit the layer cache instead of leaking a dangling image per run.

Proved end-to-end (`testdata/container_app.lua`, `tests/container_app.rs`): a real HTTP service built
from a nested Dockerfile, running on the topology network, resolving `postgres` by DNS alias, driven
black-box by the host runner over its published port — with rows inserted through the DB's *host*
vantage showing up in the SUT's answers, so both vantages demonstrably address one live resource.
Mutation-checked: swapping `db.network.url` for `db.url` fails it (`127.0.0.1` inside a container is
that container), so the proof genuinely tests the vantage rather than passing incidentally.

One latent bug surfaced on the way: `docker.run` **unconditionally pulled**, so a locally-built image
died with a misleading "pull access denied". It now pulls only when the image is not already local —
`docker run`'s own rule. That removed ~500ms of incidental latency from Proof 1, which promptly went
red and exposed a **false-ready**: `wait = { port }` probes the *mapped host* port, and Docker
Desktop's proxy accepts it before the server inside is listening (measured: the first probe after
"ready" fails). Proof 1's precondition is now an explicit `prova.retry` — the same idiom
`prova.containerized` uses for client factories. See "Remaining work".

## Remaining work (bounded, and named)

- **Per-resource addressing** — whole-topology addressing across the verbs is done; standing up or
  referencing an *individual* resource (`prova up orders.db`) is speculative, likely a non-goal.
- **Advertised-listener recipe (Kafka)** — plugin-side follow-up; the core port-mode signal is in
  place. Dual-homing is free for every resource *except* Kafka, which must advertise the network alias
  to in-network clients and the host address to host clients (`INTERNAL://kafka:9092`,
  `EXTERNAL://127.0.0.1:<host_port>`) — the one place the containerized SUT needs plugin help.
- **The archetype acceptance bar** — the mechanism is proved, but the bar named in the arc's hand-off
  is converting a *real* archetype (`dotnet-rest-service-archetype`): render → build its own
  Dockerfile → run on the topology network against `postgres.container`'s `network.url` → drive CRUD
  → cross-check the DB, dropping `requires = { "dotnet" }` for `requires = { "docker" }`. That work
  lives in the archetype suites, and is what will exercise the build-cache story on a real toolchain.
## Readiness is a contract (done)

`wait` now means what it says: when `docker.run` returns, a client's **first probe succeeds**.

It did not before, and the old behavior is worth recording because the failure was invisible.
`wait = { port }` connected to the **mapped host port** — and Docker Desktop's port proxy binds and
accepts the moment the container starts, before anything inside is listening. So the check passed
while the server was still booting. Worse, it could not fail *at all*: a container running
`sleep 120`, listening on nothing, was reported ready. The signal was not weak, it was vacuous. It
also could not see an **unpublished** port, so an in-network-only resource — a legitimate topology
member a containerized SUT talks to by alias — was not waitable.

The fix asks the **container's own kernel** instead of the host: `/proc/net/tcp{,6}` reports what the
process inside actually bound (state `0A` = LISTEN). It rejects **loopback** binds, because a server
bound to `127.0.0.1` inside a container answers only itself — not a sibling, not the host — which is
exactly the case an init phase presents when it binds localhost before the real start. Where the
image cannot answer (no `cat`/procfs — scratch or distroless), it falls back to the old host-port
check rather than failing: coarse, but no worse than before, and not misrepresented as a true signal.

Proved by `testdata/docker_readiness.lua` / `tests/docker_readiness.rs`, whose bar is deliberately
margin-free: the prober container is started **before** the database, so no container-start latency
pads the gap, and every probe is a **single attempt** with no `prova.retry`. Three parts — the first
probe succeeds; an unpublished port is still waitable; a container that never listens times out
rather than being waved through.

The corroboration: Proof 1 had briefly carried a `prova.retry` to paper over the false-ready. With a
true signal that workaround was **removed**, and the proof passes on the first attempt. Fixing a
signal should let you delete the compensation built around it — that it did is the evidence the fix
is real rather than a differently-shaped guess.

## The discipline this imposes now

The immediate substrate work — `container:run`, `prova.parse.*`, the `prova.containerized`
enhancements, the plugin registry — serves **both** verbs, so it is foundation, not detour. The single
rule it adds: **keep the topology *definition* decoupled from the terminal *verb*.** A resource/topology
must be expressible independent of a test scope, so `up` can consume it without a rewrite. Get that seam
right and `prova up` slots in cleanly; blur it and env-mode becomes a fork.

## Positioning

Hold the broad identity internally; market the sharp wedge. **Acceptance testing with real resources**
is where the pain is acute and the buyer obvious — winning it proves and funds the substrate. `prova up`
is the reveal that turns "a great test runner" into "the single tool for ephemeral environments you can
both inhabit and verify." Working identity:

> **Prova — a programmable engine for ephemeral resource topologies you can test, inhabit, or watch.**

Testing is the first consumer, the wedge, and the thing that keeps it honest: an environment you can't
assert against is just infrastructure; the assertion is what proves it's *right*.
