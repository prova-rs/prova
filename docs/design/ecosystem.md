# Plugin Ecosystem

Drafted 2026-07-13. The north-star for how Prova's capability surface grows — the strategy layer
above the mechanics in [plugin-system.md](plugin-system.md) and the grammar in
[namespacing.md](namespacing.md). Where plugin-system.md answers *how a plugin loads*, this answers
*what the ecosystem is, how capabilities are tiered, and how they progress.*

## The thesis: batteries **and** an ecosystem

"Batteries included" alone can never match pytest's flexibility — a fixed binary is a ceiling. A pure
plugin ecosystem alone loses the thing that makes Prova pleasant: a small, consistent, fast, curated
core. So Prova is **both**, cleanly separated: a curated core of first-class integrations *plus* an
open ecosystem the core cannot bound. The rest of this doc is the machinery that keeps those two from
fighting.

Five ideas, converged, carry the whole design:

1. **Names are decoupled from sources.** `require` takes a name; the manifest/registry maps it to a source.
2. **Three layers.** Primitives (always in) · native clients (feature-gated) · recipes (Lua).
3. **Black-box shrinks "native" to a convenience.** You can test a technology through the app under test.
4. **A client has three implementation strategies.** Native · dockerized-CLI · through-the-app.
5. **Two tiers behind one grammar.** First-class native · open Lua — the grammar hides which.

---

## 1. Names are decoupled from sources

A test references a **name**, never a source:

```lua
local redis  = require("redis")     -- bundled battery, or overridden in prova.toml
local rabbit = require("rabbitmq")  -- resolved via [plugins] / the registry
```

You never write a git URL in `require`. The name→source mapping lives in `prova.toml` (or a registry),
so plugins are swappable, pinnable, and mockable without touching test code. `require` returns the
**namespace table**; a bundled namespace is also injected as a global (no `require` needed) for
backward compatibility, and a declared plugin can opt into global injection (`global = true`) to feel
first-party.

### Resolution ladder (source model, mirrors archetect)

| Tier | You write | Resolves to |
|---|---|---|
| Bundled | `require("redis")` | the built-in namespace |
| Explicit source | `redis = { git = "https://…", tag = "v1" }` | that repo, pinned |
| Org/repo shorthand | `redis = "acme/prova-redis@v1"` | `github.com/acme/prova-redis` (default host) |
| Registered orgs | `[sources] acme = "github:acme"` → `"acme:redis"` | a prefix → an org |
| Registry / catalog | `redis = "^1.2"` | an index repo maps name → canonical repo + version |

Build 1–3 first, 4 when a second org appears, 5 (the `prova-rs/registry` index) when the plugin count
earns it. `prova.toml [plugins]` is the **canonical, committed, pinned** source of truth; the GitHub
Action just runs `prova` (reading it) and adds value by **caching `~/.cache/prova/plugins`** keyed on
the manifest hash, plus an optional `plugins:` input for CI-only extras.

---

## 2. Three layers

A capability is never "a plugin" or "a battery" as a whole — it is split across layers, and the split
is where the confusion dissolves. `postgres` is the worked example:

| Layer | What | Distribution | `postgres` piece |
|---|---|---|---|
| **0. Primitives** | `docker`, `shell`, `fs`, `net`, `http`, `prova.retry`, `ctx:manage` | Always bundled — the substrate | — |
| **1. Native clients** | sqlx, redis, kafka, pulsar, grpc, s3, future amqp/nats/mongo | **Feature-gated; bundled by distribution** | `postgres.client` (native, must compile in) |
| **2. Recipes** | `postgres.container`, `redis.container`, … | Lua — common bundled, long tail external | `postgres.container` (a plugin — nothing magical) |

So "is postgres a plugin or a battery?" is malformed: the **recipe is a plugin (Layer 2)**, the
**client is a native capability (Layer 1)**. The native boundary is a law of physics (you cannot
`git clone` a Rust crate into a static binary), so Layer 1 is compile-time by necessity — the only
choice is *one fat default binary vs. modular distributions* (see Distributions).

---

## 3. Black-box shrinks "native" to a convenience

The native client does two jobs — **provision readiness** and **direct state assertion**. In a
black-box world neither is mandatory:

- Readiness → a **port TCP-connect or log-substring** (`docker.run{ wait = {…} }`), no client.
- State → **inferred through the app under test**: write via its API, read via its API, assert the
  response. If it round-trips, it persisted. You never touched the database.

So a native client is a **power-up for *direct* assertion, not a requirement for testing the tech.**
The common Layer-1 clients earn their bundle because direct assertion is *ergonomic*, not because
you can't test without them; the long tail needs no bundled client at all. Two consequences baked
into the design:

- **Decouple `X.container` from `X.client`.** Provisioning + readiness need only `docker` (universal);
  `X.container` returns `{ url, container }` always and attaches `client` only when that capability is
  present. Readiness falls back to a port/log probe when the client is absent.
- The pressure to bundle rare native clients mostly **evaporates** — it becomes "drive through the app."

---

## 4. Three client strategies

"Client" has three implementations; only one touches native code:

| Strategy | How | Needs | Trade |
|---|---|---|---|
| **A. Native** | lapin/sqlx/rdkafka compiled in | the compiled feature (Layer 1) | fast, typed; heavy build + per-platform native compile |
| **B. Dockerized CLI** | Lua shells a client CLI in a container | just `docker` (a primitive) | slower, text-parsed; **cross-platform for free** |
| **C. Through-app** | assert via the app's own API | nothing | universal; sees only what the app exposes |

**B's sharpest form: exec the CLI already in the service image.** When you *provisioned* with
`X.container`, the service image usually ships its own CLI, and we already have `container:exec()`:

```lua
local pg = require("postgres")
local db = pg.container(ctx)
db.container:exec('psql -tAc "select count(*) from orders"')   -- psql is IN the image
```

`redis`→`redis-cli`, `rabbitmq`→`rabbitmqadmin`, kafka→`kafka-console-*`. Direct assertion with **zero
native code, zero extra image, no networking to arrange** (you exec inside the container you started —
sidestepping the `host.docker.internal` wrinkle a *separate* client container would hit). A `rabbitmq`
plugin doing exactly this is writable today over existing primitives.

This retracts native (A) to the genuinely small set that needs **throughput or typed streaming** (load
tests; hot typed paths like sql/redis/grpc). Kafka illustrates: **B** (`kafka-console-producer` exec)
for functional assertions in the default build; **A** (rdkafka) only in `prova-full` for load
scenarios — offer both, keep the default light.

---

## 5. Two tiers behind one grammar

Native integrations survive black-box + docker-wrappers not for *capability* but for **quality and
consistency**: the Kafka≈Pulsar symmetry (identical facets, learn-one-know-all) is a product feature a
free-for-all pile would lose. So the ecosystem is tiered:

- **Tier 1 — first-class native integrations.** Beautiful, consistent, fast, typed. Curated by us,
  designed in **families** (messaging: kafka/pulsar/nats · sql: postgres/mysql/sqlite · object-store:
  s3/gcs/azure) so within a family the API is the same API with a different backend.
- **Tier 2 — pure Lua plugins** (docker-exec or through-app). Open, long-tail, cross-platform free.

**The grammar is the tier-agnostic interface — and that is the trick.** At the call site you cannot
tell native from docker-wrapper:

```lua
local kafka = require("kafka")
local mq = kafka.container(ctx)   -- { client, url, container } — same shape either way
```

The **tier is an implementation detail the grammar hides.** Ergonomics are uniform because the
*interface* is the contract and the strategy is hidden beneath it.

### Making conformance the path of least resistance

Tier 2 *inherits* Tier 1's feel through a **scaffolding helper** (`prova.containerized`) that makes
the grammar-shaped thing the easy thing — the author supplies only the tech-specific bits and gets
`ctx:manage`, retry-readiness, and the `{ client, url, container }` trio for free:

```lua
return prova.containerized{
  name  = "rabbitmq",
  image = "rabbitmq", tag = "3", port = 5672,
  url    = function(hp) return "amqp://127.0.0.1:"..hp end,
  client = function(url) return ... end,   -- native? docker-exec? omitted (black-box)? — the only line that varies
}
```

The *same helper* yields a conformant namespace whether `client` is native, a `container:exec`
wrapper, or absent — so native-vs-docker collapses to "what does `client` do," and every plugin comes
out grammar-shaped by construction. An optional `prova plugin lint` (checks facets/trio) keeps the
ecosystem coherent without a gatekeeper. When the beautiful thing is the default thing, consistency is
emergent, not enforced. **First-party recipes are authored through this same helper** — the dogfood
proof that the seam is real.

### Tiers are a maturity gradient, not a caste

Because the interface is identical, a capability can **start** as a Tier-2 docker-exec plugin (ship in
a day) and be **promoted** to a Tier-1 native family member once it earns it by frequency or
throughput. Nothing at the call site changes on promotion — only the hidden strategy. That pipeline is
what lets "batteries + ecosystem" stay consistent instead of sprawling.

---

## Plugin shapes

Everything above (facets, the trio, Docker) describes the **resource** shape — but that is one shape,
not the definition of a plugin. **The only universal contract is: a plugin is a Lua module that
`return`s a table.** `prova.containerized` is a *constructor* for the resource shape; it sits at one
row of this table, not at the root.

| Shape | Returns | Docker? | Constructor | Example |
|---|---|---|---|---|
| **Resource** | `{ client, url, container }` | yes | `prova.containerized` | `rabbitmq`, `postgres` |
| **Client-only** | a client factory (attach, no provisioning) | no | — (none yet) | attach to an external Stripe / dev-cluster over `http` |
| **Library** | an arbitrary table of functions | no | — (none needed) | JWT/token DSL, data builders, custom matchers, a company auth-flow helper |
| **Composite** | a higher-level flow over several resources | via its parts | — | "spin up the whole stack" |

Consequences that follow from "a plugin is any namespace":

- **`requires = { "docker" }` is a property of the *resource* shape, not of plugins.** A library
  plugin needs nothing — it is just Lua the searcher resolves and `require` returns.
- **`prova plugin lint` classifies; it does not prescribe.** It fails only on what is wrong for *any*
  plugin — a non-table return, or a resource facet (`client`/`container`/`wait_for`) that is present
  but not a function. "No resource facets" is not an error; it is the signal for a **library** plugin.
  Lint reports the shape (`resource` / `library`), never rejects a valid library.
- **Restraint on constructors.** Libraries need no constructor (nothing to abstract); `prova.containerized`
  earns its place because the resource shape has real boilerplate (provision + wait + manage + trio).
  A second constructor is added only if a shape proves to carry recurring boilerplate — client-only
  (attach + readiness) is the likeliest future candidate, but not yet.

So the plugin *system* is **Lua**; **Docker is the substrate for the resource shape specifically**,
which is the most common shape but not the only one.

---

## Unified `requires`

There is **one** capability gate. `requires_native` is not a separate concept — it is the same gate
with a different detector. A capability is a name resolved through a registry of detectors:

| Capability | Detector |
|---|---|
| `docker` | `docker info` succeeds (environment probe) |
| `github` | `GITHUB_TOKEN` set |
| `git`, `cargo`, `kubectl` | binary on `PATH` |
| `kafka`, `postgres`, `amqp` | **compiled into this build** (`cfg!`-assembled set) |
| *(future)* `acme.thing` | the plugin resolved/loaded |

`requires = { "kafka" }` skips in a `prova-min` build exactly as `requires = { "docker" }` skips
without a daemon — same code path, same cascade, same reason. When a native capability is absent, a
**stub namespace** makes `kafka.client(...)` raise *"the kafka client isn't in this build (needs the
`kafka` feature / a fuller distribution)"* instead of a nil-index. So: **declare** `requires` → graceful
skip; **forget and call** → a clear, actionable error.

## Distributions

Feature flags are the distribution knobs; the binary's Layer-1 set is a build choice:

- `prova-min` — primitives only. Tiny.
- `prova` (default) — primitives + common native clients (sql/redis/http/grpc/docker) + common recipes.
- `prova-full` — every native client, for the long tail.
- custom build — pick your Layer-1 set.

The homebrew tap ships variants. Requiring a recipe whose native client isn't in *your* distribution
skips (declared) or errors clearly (called) — how a rare native client is reachable without bloating
everyone's binary.

## The `prova-rs` org

- `prova-rs/prova` — the binary (ships bundled batteries).
- `prova-rs/prova-<name>` — official plugins (`prova-redis`, `prova-postgres`, …): both bundled *and*
  published standalone, so they are the canonical authoring examples and are pin/override-able.
- `prova-rs/registry` — the plugin index (name → repo → versions), when the count earns it.
- `prova-rs/run-action` — the GitHub Action (manifest-canonical + plugin cache).
- Community plugins live anywhere, referenced by shorthand or listed in the index.

## Native plugins — the heavyweight future hatch

For the narrow case of "direct assertion against a technology no distribution bundles and can't be
inferred through the app," native plugins (dynamic extension, à la Substrate's extension system) are
viable but cost a **cross-platform release matrix per plugin** — tedious enough that, given §3–4, it is
the rare escape hatch, not the common path. Kept on the roadmap, Substrate-informed; not led with.

## Roadmap

1. **`prova.containerized` scaffolding helper** — the ergonomic keystone both tiers depend on; dogfood
   by re-expressing first-party recipes through it. **(done)**
2. Unified `requires` compiled-capability detector + stub namespaces. **(done)** — a native
   capability (`kafka`, `postgres`, …) resolves by whether its feature is compiled in, so
   `requires = { "kafka" }` skips in a lean build exactly as `requires = { "docker" }` skips without
   a daemon; an absent namespace is a stub raising a clear "not compiled into this build" error.
3. Org/repo shorthand resolution (reusing the git fetch) → registered orgs. **(done)** — a string
   plugin source is classified: a git URL, a `host:org/repo[@ref]` shorthand (`github`/`gh`,
   `gitlab`/`gl`, or a `[sources]` alias), or a bare `org/repo@ref` (defaults to github; the `@ref`
   is required so a plain path is never a surprise fetch) → a git source; anything else → a local
   path. `[sources]` registers aliases (`acme = "github:acme"` → `acme:redis` = github.com/acme/redis).
4. Action: plugin cache + `plugins:` input. **(done)** — `prova-rs/run-action` caches
   `~/.cache/prova/plugins` (keyed on the manifest) so pinned plugins clone once, and its `plugins:`
   input (one `name = source` per line) expands to prova's repeatable `--plugin name=source` flag,
   layered over the manifest.
5. Stand up an external plugin (dogfood the round-trip); `prova plugin lint`. **(done)** —
   `prova-rs/prova-rabbitmq` is the first standalone plugin: a **zero-native-code** RabbitMQ resource
   (docker-exec over `rabbitmqadmin`, authored through `prova.containerized`), self-testing through
   Prova against a live daemon. `prova plugin lint <file>` checks a plugin returns a namespace with
   grammar facets (`client`/`container`/`wait_for`, each a function). *(Chose rabbitmq over redis: a
   true external technology Prova doesn't bundle, so it exercises the real docker-exec path — a
   stronger dogfood than re-exposing a bundled recipe.)* Also: `prova.containerized`'s `client`
   factory now receives the `container` (`client(url, opts, container)`) so docker-exec clients can
   `exec` into it — the "fix the starter first" change this plugin surfaced.
6. The `prova-rs/registry` index; distributions (`prova-min`/`prova-full`) + tap variants.
7. Later: native-plugin hatch, if a real need appears.
