# Topologies — one definition, multiple consumers

Drafted 2026-07-14. The north-star that reframes what Prova *is*. Where [ecosystem.md](ecosystem.md)
covers *wrapping* resources and [architecture.md](architecture.md) covers the test runner, this
records the larger identity those two pillars fuse into — and the design seam that makes it real.

## The identity

Prova is two platforms welded together:

1. **A test runner** — fixtures, the dependency DAG, the scheduler, assertions, reporters, isolation.
2. **A resource-orchestration layer** — provision ephemeral infra, wire it, drive it, tear it down.

The weld is **the grammar** (`{ client, url, container }`, `ctx:manage`, `requires`, `prova.retry`):
every resource — bundled or package, native or docker-exec, a database or a whole Kubernetes topology
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
- **Live component development** — hand the held architecture to a tool that already does
  rebuild-and-redeploy well (Tilt), rather than growing one. *(direction — see below)*

The point is not "it does both." It is that the **same definition powers your tests and your dev
environment, so they cannot drift.** Today a compose file, a testcontainers setup, k8s manifests, and
test fixtures are four separate descriptions of "the same" environment that silently diverge. Prova
collapses them to one. No existing tool does this, because they are separate tools.

```lua
-- The factory, exported from a package so both doors can reach it (see §Two doors below).
function kitchen.orders(ctx)
  local db  = require("postgres").container(ctx)
  local mq  = require("kafka").container(ctx)
  -- The SUT is a container, so it is wired with the IN-NETWORK vantage — `db.network.url`
  -- (alias + container port), never `db.url` (127.0.0.1 + mapped port). Inside a container,
  -- `127.0.0.1` is that container; see §The containerized SUT.
  local app = boot_app(ctx, { db = db.network.url, kafka = mq.network.url })
  return { db = db, mq = mq, app = app }
end

local env = prova.topology("orders", kitchen.orders)

prova.test("an order lands in the DB", function(t)
  local e = t:use(env)                        -- test: instantiate → drive → assert → teardown
  e.app:post("/orders", { sku = "A1" })       -- the runner drives it over the HOST vantage
  t:expect(e.db.client:query_value("select count(*) from orders")):equals("1")
end)
```

```toml
[topologies]
orders = { package = "kitchen", topology = "orders" }
```

```
prova                # runs the assertions against `env`
prova up orders      # stands up the SAME factory, prints endpoints, holds until Ctrl-C
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

## Two doors — registration is the whole surface for the inhabited verbs (landed 2026-07-28)

A topology has exactly two consumers, and they enter by different doors. A **test** builds one
in-process (`prova.topology(...)`); the **inhabited verbs** stand up a **registered** factory.
`up`/`start`/`down`/`ps` originally did both — loading every proof file and standing up any
`prova.topology` call found there — and two problems followed.

A test-local fixture became silently addressable as a shared environment, which is not what
declaring a fixture inside a test file means. And the two sources collided: registering a topology
in `[topologies]` *and* declaring it in a proof — the natural thing when one package is both a
package and its own suite, which is exactly the reference kitchen sink — aborted with
`topology "x" is already defined`, an error that never mentioned that one registration came from
the manifest. The only way to have both verbs work was to pick one and lose the other.

<!-- claim: registration-is-the-only-door -->
`[topologies]` is now the whole surface for the inhabited verbs; **no files are loaded**. The two
doors stop competing, so a package can register a topology for `prova up` and build the same
factory as a fixture in its proofs — one definition, addressed twice, unable to drift. The
`requires` gate also becomes universal: a code-declared topology carried no advertisement and so
stood up ungated, where a registered one inherits the environment requirements its package
advertises.

<!-- claim: test-only-topology-is-not-addressable -->
Breaking, deliberately, and pre-1.0: a topology declared *only* in a test file is no longer
visible to `up`. The failure says so and prints the `[topologies]` entry to add, rather than
reporting "no topologies defined".

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
<!-- claim: up-self-registers -->
- **`prova up <name>` (attached)** — **done.** Resolves the named topology from `[topologies]`
  (see §Two doors — it does **not** load proof files), provisions it under a held File scope,
  prints each resource's `url`, and blocks until **SIGINT or SIGTERM**, then runs the existing
  `ctx:manage` teardown. Verified with a real Postgres container
  (endpoint on a live host port; container reaped on Ctrl-C). A running `up` **self-registers** a
  record under `<home>/.prova/var/running/<name>.json` (pid + endpoints; self-gitignored) and removes it on
  clean teardown.
<!-- claim: detached-supervises-attached -->
- **Detached mode** (`prova start` / `prova down` / `prova ps`) — **done**, and exactly the thin
  **supervisor over attached `prova up`** the design predicted: `start` spawns `prova up <name>` in
  its own process group (stdio → `<home>/.prova/var/running/<name>.log`), waits for it to self-register, prints
  the endpoints, and returns leaving it running; `down` reads the record and `SIGTERM`s the pid, so
  the *same* in-process `ctx:manage` teardown runs in the detached child; `ps` lists records (cleaning
  stale ones). **One provisioning path, one teardown path** — no resource-inventory tracking, no
  survive-process-exit container semantics, no second teardown implementation. Verified end-to-end with
  a real Postgres container (survives `start` returning; reaped by `down`) and a no-docker CLI
  integration test proving the detached child runs teardown on `down`.
  **`start` is not silent while it waits**: it relays the holder's log to its own stderr as the
  holder writes it, stopping at the endpoints block so that block is still printed once, on stdout
  (agent-ergonomics.md#start-shows-what-up-shows). And **Ctrl-C during a `start` stops the holder**
  rather than orphaning it — the supervisor is the only process that can hear the signal, since the
  holder deliberately sits in its own group (agent-ergonomics.md#interrupt-leaves-nothing-behind).

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
question for the common case; the Kafka advertised-listener recipe is a package-side follow-up that
now has the core signal it needs.

## `prova watch` — removed (2026-08-25)

`prova watch <name>` promised the Tilt-ish loop: stand the topology up and re-provision whenever the
definition changed. **It never did that.** The re-apply was driven by mtime polling over the file
list `build_topology_run` returns, and when "registration is the only door" landed (2026-07-28) that
list became `Vec::new()` for every inhabited verb — correctly, since a `[topologies]` entry is now
the whole surface and no proof files are loaded. `up` does not care what is in `files`; `watch`'s
entire premise was in there. The comparison became `[] != []`, which is never true, so the loop could
not fire. It shipped that way for a month, advertised in the CLI help, the learn card, the skill
card and the public docs site, and nothing tested it — the one unit test asserted the *unknown
topology* path and its comment claimed the happy path was "verified via the CLI", which it was not.

Worse than dead: it had no double-provision guard. `up` refuses when a live record exists; `watch`
went straight to the factory, so running it beside a held topology provisioned a **second** instance.
On a fixed-name definition that is the collision [[fresh-over-a-holder-is-announced]] warns about —
observed for real against a live `ybor-studio` kind cluster, where `kind create cluster --name
ybor-studio` failed with "node(s) already exist". The cluster survived only because that definition
registers its `ctx:defer(kind delete cluster)` *after* the create returns; register the cleanup first
— the more defensive-looking order — and the interrupted watcher's teardown would have deleted a
cluster another process was holding. It was also invisible to `ps`/`down`, having never registered
run-state.

It was removed rather than repaired, and removed **silently** (no tombstone): it had never been run
in anger, so there is no muscle memory to be kind to.

### What replaces it: hand off, don't rebuild

The need behind `watch` is real and is two needs wearing one name:

1. **The topology definition changed** — a service added, seed data edited. That is a
   re-provision, not a hot reload. `prova down && prova start` is the honest spelling, and
   pretending otherwise was most of what made `watch` a lie.
2. **An application inside the architecture changed** — the component under active development.
   This is rebuild-image-and-redeploy-into-the-running-cluster, with file watching, live update and
   port forwarding. **Tilt already does this well, and prova should not grow a second one.**

So the intended shape: **prova owns the architecture, Tilt owns the components under development.**
`prova start` brings up the whole thing (cluster, datastores, identity, gateway); Tilt then replaces
one or two deployments in that same cluster while everything around them stays put. Both point at
one environment, and `prova down` remains the single reaper.

What prova owes that handoff — none of it built yet, and this is the bounded list:

- **A machine-readable "where is it".** The holder already records the factory's JSON projection in
  `<var>/running/<name>.json` (the same payload an attaching run seeds — [[attach-binds-by-name]]),
  but nothing emits it for a consumer: `prova ps` is human text only. A Tiltfile should be able to
  ask prova for the kube context / kubeconfig / endpoints rather than re-deriving them by
  convention.
- **A convention for what a k8s topology returns**, so that answer is uniform across packages
  rather than each Tiltfile knowing one factory's shape.
- **The double-provision guard on every inhabited verb**, which is the defect above and is owed
  regardless of Tilt.

Explicitly not prova's job: building images, live-updating containers, watching source trees.

Note the symmetry worth keeping: the MCP warm path (`up` → `run`/`eval` → `down`) is the *agent's*
interactive loop over a held topology, and Tilt is the *human's*. Two consumers, one held
architecture, no second provisioning path.

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

## Held-topology attach — one capability, every holder (done)

The MCP server's warm re-runs and the CLI's detached topologies were two halves of one idea: a
**held topology** is a named, running instance any run may bind instead of provisioning. The warm
path already had the whole mechanism in-process — inject the held value into the fresh run's scope
caches keyed by topology *name*, register no teardown, let the holder reap (`run_warm`). Attach is
the cross-process sibling: the holder records a **JSON projection** of the factory's returned value
in its `running/<name>.json` record, and an attaching run **rehydrates** that projection and seeds
it at the same seam. Closures and userdata do not survive the projection, and must not — the
resource grammar's standing answer is that clients attach by `url`.

Testing the *current state of things* is the point: hold the stack, swap a work-in-progress SUT
into it (Tilt, by hand), and the same suite that gates CI judges the live instance.
Idempotency under accumulation is **test design**, not framework mechanics — upsert-or-error
seeding, count-then-delta assertions, and a `[profiles.live]` lane are the sanctioned tools.

<!-- claim: attach-binds-by-name -->
A plain `prova` run, finding a LIVE held record whose name the collection declares
(`prova.topology(name, …)`), binds the fixture to the held instance: the factory does not run, and
`t:use` resolves the rehydrated value. The attachment is announced on stderr — an attached run is
deliberately non-hermetic, and that is never silent. A held name the collection does not declare
is skipped; a stale record (dead pid) is ignored.

<!-- claim: attach-leaves-holder-sovereign -->
An attached run registers no teardown for the held value: the holder remains the one true reaper,
exactly as in the warm path. Runs come and go; `down` (or the holder's signal handler) is the only
place the environment dies.

<!-- claim: fresh-opts-out -->
`prova --fresh` ignores held records entirely and provisions per the fixture — the CI behavior,
on demand. `--fresh` with `--topology` is a contradiction and is refused.

<!-- claim: require-topology-is-strict -->
`prova --topology NAME` **requires** the attachment: it is an error when no live holder of that
name exists, and an error when the suite never declared the name — a run meant to judge a live
environment (a Tilt-injected build) must never quietly test something else. This is the CLI mirror
of the MCP rule that warm calls never provision implicitly.

<!-- claim: attach-is-recorded -->
The run record carries `attached: [names]` — live-state evidence is legitimately weaker than
hermetic evidence (the environment predates the run and may carry state from prior ones), so the
provenance is durable where `attest`/`evidence` read.

## Run-wide topologies — the run is a holder too (done)

A `prova.topology(...)` in a proof file is a **fixture**, local to the files that declare it. So a
package whose proofs span several files, each declaring the same registered topology, built that
environment **once per file**. Measured on ybor-studio: three proof files declaring the docker
topology and two declaring the kind topology turned an eleven-container world into **33 container
creations plus a cluster** for one bare `prova` — 364s to answer one question. The machine-scoped
locks serialized the duplicates, which made the waste safe and also made it slower.

Held topologies already dedupe — every file attaches to the one live instance — so the field
workaround was to hold first (`prova start <name> && prova`). That inverts the promise of the cold
path: CI and a fresh checkout pay N× for the suite the author runs warm, and nothing in the output
says so, because each provision narrates independently and nothing frames the repetition as
repetition.

<!-- claim: run-wide-topology-is-provisioned-once -->
A `[topologies]` entry may declare **`scope = "run"`**: the topology is provisioned **once for the
whole run** and every declaring file binds that instance. The run itself becomes the holder — a
pool thread outside every suite owns the instance, hands each declaring state the same JSON
projection attach uses, and reaps it after the last suite. Evidence stays hermetic: the run created
what it tested, so this is not an attachment and the run record carries none.

<!-- claim: file-local-is-still-the-default -->
Absent the key, nothing changes: a topology is file-local, provisioned per declaring file, exactly
as before. The opt-in is deliberate and belongs to the package rather than the invocation — a
run-wide environment **accumulates state across files**, and the value each file sees is the JSON
projection (closures and userdata do not cross a Lua state; clients attach by `url`). Only the
package's own author can weigh that against N× provisioning.

<!-- claim: unknown-sharing-scope-is-refused -->
A `scope` value that is neither `"run"` nor `"file"` is **refused**, naming both honorable values,
whether or not this run would have used that topology — never quietly read as file-local, which
would hand back N× provisioning under a key that says otherwise
(docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored).

<!-- claim: run-wide-is-still-demand-driven -->
Run-wide does not mean eager. Nothing is provisioned until a test asks, so `-k` on one unrelated
proof pays nothing — the property that lets an honestly-minutes environment be declared run-wide
without taxing every narrow run.

<!-- claim: run-wide-provisioning-is-single-flight -->
<!-- claim: run-wide-failure-is-memoized -->
Provisioning is **single-flight** across workers, the same shape a `Scope.Run` conduct uses
(docs/plans/shared-deputies.md): whoever asks first claims the slot, everyone else waits on it —
and the wait is narrated, because it lands inside the waiting test's own duration. A failure
settles the slot exactly as success does, so a second file replays the recorded error as a memoized
verdict instead of re-paying a provision that just failed
(docs/design/lifecycle.md#fixture-failure-memoization).

**Why the definition comes from `[topologies]` and not from the file.** The holder rebuilds the
factory in a fresh Lua state, and the only definition a fresh state can reach is the registration
(`require("<package>").<factory>`) — a closure living in a proof file cannot travel to another
thread, and neither can the teardowns a factory parks on its scope. That is also why this cannot be
written in user-land: each proof file evaluates in its own state, and `require` cannot smuggle one
live instance across them. A file's declaration of a run-wide name is therefore the **demand**, not
the definition — precisely as it is when a run attaches to a detached holder.

<!-- claim: attach-outranks-interning -->
A live holder still wins. An attached instance is seeded before any fixture resolves, so a run
declaring a run-wide name attaches to the detached holder rather than provisioning its own, and it
reaps nothing it did not create. Ownership stays a single rule at every layer: the holder reaps.

<!-- claim: fresh-over-a-holder-is-announced -->
`--fresh` beside a live holder is **announced**. It means "provision my own", which is harmless for
a definition whose resources are per-instance (two random-port stacks coexist) and destructive for
one that is not: a fixed-name cluster (`kind create --name ybor-studio`) collides on creation, and
then this run's teardown reaps the *holder's* cluster, because both spell the same name. Prova
cannot see from here which kind of definition it has, so it warns rather than refuses, and names
both exits (`prova down <name>`, or drop `--fresh`). Run-wide sharing reduces the multiplicity of
that hazard; it does not remove it, which is why the warning is its own thing.

## Remaining work (bounded, and named)

- **Per-resource addressing** — whole-topology addressing across the verbs is done; standing up or
  referencing an *individual* resource (`prova up orders.db`) is speculative, likely a non-goal.
- **Advertised-listener recipe (Kafka)** — package-side follow-up; the core port-mode signal is in
  place. Dual-homing is free for every resource *except* Kafka, which must advertise the network alias
  to in-network clients and the host address to host clients (`INTERNAL://kafka:9092`,
  `EXTERNAL://127.0.0.1:<host_port>`) — the one place the containerized SUT needs package help.
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
enhancements, the package registry — serves **both** verbs, so it is foundation, not detour. The single
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
