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
- **Plugin ecosystem** — published packages under `prova-rs/`: prova-redis, prova-kafka,
  prova-s3, prova-postgres, prova-mysql, prova-pulsar, prova-rabbitmq (+ prova-plugin-archetype,
  run-action). **prova-mongodb** was authored this session (green self-test) but is **not yet
  published**.
- **Plugin LuaCATS/IDE support** — done and pushed: declaring a plugin in `prova.toml` auto-syncs its
  annotation stub so `require("<name>")` completes in-editor with zero manual wiring.

## Plans — topology + ecosystem resolved; parallels (A) done; mocks in flight

- [parallels.md](parallels.md) — VM-style testing, and the Linux proving ground C2 needed. **(A) the
  Linux harness (`scripts/vm-linux-proof.sh`) — done**, proving C2 on a native-Linux VM; **(B) a
  `parallels.vm(ctx)` resource plugin — deferred** until VM-style testing has a real consumer. Records
  the new axis C2 exposed: *where prova runs relative to the substrate*.
- [mocks.md](mocks.md) — virtualize the dependency you can't run, and assert on the interactions you
  can't otherwise see. **Open — A (`http.mock`), B (`grpc.mock`), C1 (passthrough/record/replay), and
  C2 (the network vantage) landed 2026-07-16/17; C3 + D + E open.** `mock` is a fourth facet,
  core-native rather than the plugin `foundations.md:154` assumes, with passthrough/record/replay as
  one option on the same object.

  The load-bearing bet held twice: **a stub's reply can be a Lua function**, run on the live Lua state
  while the coroutine driving the SUT is suspended — over HTTP/1 *and* over HTTP/2, so there is no
  response-templating language, now or later. B settled generalization (the facet's shape carried to
  a second protocol unchanged); C1 added the observe dial (record real traffic, replay it hermetically
  — the drift answer); C2 gave a host-bound mock a `.network` vantage a containerized SUT reaches,
  **proved on a native-Linux VM** where a `127.0.0.1` bind genuinely fails (the mutation that Docker
  Desktop hides). A raising reply handler now fails its scope (`allow_handler_errors` opts out), and
  teardown errors are reported rather than swallowed.

  **Next: C3** — alias interposition (the shim), now unblocked by the same VM harness; then D
  (`net.mock` / unix sockets) and E (`graphql.mock`), each behind a real-consumer trigger. C3's shim
  builds locally via `docker.build` (a dumb TCP forwarder — nothing to version), so `release.yml` is
  untouched. Details in the plan.
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
