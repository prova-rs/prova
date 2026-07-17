# In-progress plans

Working plans for active, in-flight prova work — distinct from `docs/design/`, which holds the
durable design docs (north-star, architecture, ecosystem, topologies). When a plan lands, fold the
outcome into the design docs and trim it here.

> **Repo location note:** the accumulated project memory (build history + North Star) still refers to
> the original checkout at `/Users/jimmie/personal/archetect/prova`. As of 2026-07-15 the active
> working copy is **this repo** at `/Users/jimmie/personal/prova-rs/prova`.

## Snapshot — 2026-07-15

- **prova `v0.2.3`** is the latest release; workspace is at 0.2.3 plus three unreleased-at-the-time
  **topology** commits (now rebased onto `origin/main` and pushed): `prova.topology` + `prova up`
  (attached), `prova start/down/ps` (detached), and an `examples/topology/` dogfood.
- **Native resource clients fully extracted** — redis, kafka, s3, postgres, mysql, pulsar are all
  external docker-exec plugins. Core native surface is now only: `docker` (bollard substrate),
  `http`/`grpc`/`graphql` (network-drive primitives), `yaml` (parse util), `sqlite` (embedded, no
  docker).
- **Plugin ecosystem** — published plugin repos under `prova-rs/`: prova-redis, prova-kafka,
  prova-s3, prova-postgres, prova-mysql, prova-pulsar, prova-rabbitmq (+ prova-plugin-archetype,
  run-action). **prova-mongodb** was authored this session (green self-test) but is **not yet
  published**.
- **Plugin LuaCATS/IDE support** — done and pushed: declaring a plugin in `prova.toml` auto-syncs its
  annotation stub so `require("<name>")` completes in-editor with zero manual wiring.

## Plans — two tracks resolved, one open

- [parallels.md](parallels.md) — VM-style testing, and the Linux proving ground C2 needed. **(A) the
  Linux harness (`scripts/vm-linux-proof.sh`) — done**, proving C2 on a native-Linux VM; **(B) a
  `parallels.vm(ctx)` resource plugin — deferred** until VM-style testing has a real consumer. Records
  the new axis C2 exposed: *where prova runs relative to the substrate*.
- [mocks.md](mocks.md) — virtualize the dependency you can't run, and assert on the interactions you
  can't otherwise see. **Open — Phases A (`http.mock`) and B (`grpc.mock`) landed 2026-07-16; C–E
  open.** `mock` is a fourth facet, core-native rather than the plugin `foundations.md:154` assumes,
  with passthrough/record/replay as one option on the same object.

  The load-bearing bet held twice: **a stub's reply can be a Lua function**, run on the live Lua state
  while the coroutine driving the SUT is suspended — over HTTP/1 *and* over HTTP/2, so there is no
  response-templating language, now or later. B also settled generalization: the facet's shape
  carried to a second protocol unchanged, only the vocabulary inside the tables moved. Both are
  docker-free and network-free (26 Lua proofs, ~80ms total).

  **C is next and is the valuable one** (observe/passthrough + alias interposition — assert on real
  traffic with the real dependency). It is also the expensive one: `default@` surfaced that its host
  vantage rests on `host.docker.internal`, which reaches a `127.0.0.1`-bound server on Docker Desktop
  and **not** on Linux — so C owes a bind-address change, `extra_hosts` plumbing through
  `docker.run`/`prova.containerized`, a shim image published from `release.yml`, and a proof that runs
  on **Linux CI** (a green laptop proves nothing here). Those three are written up in the plan.
- [topology.md](topology.md) — the "one definition, multiple consumers" holy grail. **Resolved.**
  Attached + detached modes, three verb-selected port modes (fixed host ports / external reachability),
  and `prova watch` (the inhabited dev loop) all land. Remaining items are future/plugin-side and
  named in the plan (per-resource addressing, a Kafka advertised-listener recipe).
- [plugin-ecosystem.md](plugin-ecosystem.md) — **resolved.** Extraction done; mongodb published;
  examples graduated; profile-scoped plugins landed. Registry reframed as optional discovery
  (deferred) and feature-flag distributions cut — see the plan for rationale.

## Progress log — 2026-07-15 (session 2)

- **Profile-scoped plugins** landed (`[profiles.<name>.plugins]` overlays `[plugins]`).
- **Port modes / fixed host ports** landed: `RunConfig::ports` → `prova.ports`, honored by
  `prova.containerized`; `prova up/start --fixed`. Verified live (reachable on 5432/6379).
- **`prova watch`** landed: re-apply loop over the same definition, mtime-polled with a settle,
  fail-soft on bad edits, shared provisioning path with `up`. Verified live (touch → one clean
  re-apply → Ctrl-C teardown, no orphaned containers).
- Ecosystem track closed out; registry + distributions re-assessed (deferred / cut).

Both plans are now trued-up to reality. Once the outcomes are folded into `docs/design/` (mostly done),
these working plans can be trimmed to just the future/plugin-side notes.
