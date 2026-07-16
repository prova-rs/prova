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

## Plans — both tracks resolved

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
