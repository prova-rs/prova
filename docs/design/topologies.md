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

## The Holy Grail: `prova up` and `prova test` on the same definition

> **One topology definition. Multiple consumers.**

You describe a topology once — resources, wiring, how they're driven — in Lua. Different verbs consume
the *same* definition:

- **`prova test`** — bring it up, drive it, **assert**, tear down. *(today)*
- **`prova up`** — bring it up, print the endpoints, **hold it running** for you to develop against,
  tear down on signal. *(the reveal)*
- **`prova watch`** — the above plus a live re-apply loop. *(further out; Tilt-ish, not day one)*

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

## Honest remaining work (bounded, and named)

- **A held execution mode** (`prova up`): provision the topology, report endpoints, block until signal,
  tear down. Plus, later, **detach + `prova down <name>`**, which needs a little state to track what is
  running.
- **External reachability.** An inhabited environment's endpoints must be reachable by real external
  tools, not just in-container `exec`. This makes the **fixed-host-port** capability (surfaced by the
  kafka plugin: advertised listeners need a fixed port) load-bearing, where in test-only mode the
  exec client could dodge it via container-internal ports.
- **Topology addressing** — how `up` names and selects a topology to stand up (by the topology's name;
  a file may define several).

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
