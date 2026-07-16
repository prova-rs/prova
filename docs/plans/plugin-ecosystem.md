# Plan: plugin ecosystem

Design: [`docs/design/ecosystem.md`](../design/ecosystem.md),
[`docs/design/plugin-system.md`](../design/plugin-system.md). The extraction arc is **done** — all
native resource clients now live in external docker-exec plugins, generated from
`prova-plugin-archetype` and pinned via `prova.toml`. What remains is the "make it an ecosystem"
tail.

## Done

- **Native client extraction complete** — redis, kafka, s3, postgres, mysql, pulsar are all external
  plugins (zero native code, docker-exec via `prova.containerized` + `container:run`). Core native
  surface is now docker + http/grpc/graphql + yaml + sqlite only. Released as **prova v0.2.0**
  (breaking → 0.x-minor); run-action default bumped to match; all plugins pin `>=0.2,<0.3`.
- **exec-CLI SDK** — `container:run(argv|string, {stdin})`, `prova.parse.{lines,rows,table,json}`,
  and `prova.containerized` fixed-ports + `extra()`. A docker-exec plugin now writes only
  image + port + CLI commands.
- **prova-plugin.toml** manifest (entry/name/requires.prova), intra-plugin `require("<name>.helpers")`
  by canonical namespace, and the **self-contained rule** (no dependency resolver — vendor or promote
  to primitives).
- **`prova plugin lint`** — classifies plugin shape (Resource / Library) and validates facets.
- **Plugin LuaCATS/IDE support** — plugins ship a `library/<name>.lua` `---@meta` stub; prova syncs
  resolved plugins' stubs into `<home>/annotations/` and manages `.luarc.json` so `require("<name>")`
  completes with zero wiring. Baked into the archetype.

## Remaining

_None — the ecosystem track is resolved. Profile-scoped plugins landed; the two big-ticket items are
deferred/cut with rationale below._

## Deferred / reframed (2026-07-15 re-assessment)

- **Registry → discovery, not resolution (deferred).** A registry is *not* needed to resolve names —
  `prova.toml` already pins every plugin to an exact source. The genuinely useful feature it implies
  is **discovery**: an interactive `prova plugin add` that searches a catalog and lets you check the
  plugins you want, then writes the `[plugins]` entry (and pin) into `prova.toml`. That's additive and
  optional; the resolution ladder is not. Deferred until there's a catalog worth searching (today all
  plugins are first-party under `prova-rs/`). When built, it's discovery-first, not a resolver.
- **Feature-flag distributions → cut.** A single minimal kernel that is *always* present is the value
  prop: `prova` must "just work" the same way no matter where it runs. Multiple binaries
  (`prova-min`/`prova-full`) break that guarantee — a test authored against one could fail on another.
  The current minimal native surface (docker + http/grpc/graphql + yaml + sqlite) is exactly the
  kernel every prova run should be able to assume. Install variety is a packaging concern, already
  covered by the homebrew tap; it does not need build-time feature splits.

## Done (this arc, 2026-07-15)

- **`prova-mongodb` published** — GitHub repo `prova-rs/prova-mongodb`, released `v1`.
- **Profile-scoped plugins** — `[profiles.<name>.plugins]` overlays the project-wide `[plugins]`
  (profile wins on name conflict), merged in `Manifest::resolve` and flowing through
  `resolve_from_manifest` → `resolve_plugins` unchanged. CI-only capabilities now live in `prova.toml`
  so `--profile ci` and local resolve identically. (`crates/prova-cli/src/manifest.rs`.)
- **Quarantined examples graduated** — the capstone/kitchen/suite examples are all runnable now,
  using `require("<plugin>")` + `prova.toml`. The three files left in `examples/aspirational/`
  (`ordering.lua`, `dependent_flows.lua`, `http_service.lua`) are *design showcases* blocked on
  unimplemented authoring API (`f:use`, `ctx:param`), not on the ecosystem — tracked as a language
  feature, not an ecosystem cleanup.

## Non-goals (explicit)

- **No third-party native/binary plugins.** Native code is first-party-bundled only (the network-drive
  trio + docker + sqlite). Lua + docker + black-box-through-app covers the plugin space.
- **No dependency resolver.** Unsatisfiable in one shared Lua state; vendor helpers via canonical
  namespace, or promote a widely-wanted capability into prova primitives (T2→T1).
- **TLS / secured-remote-endpoint support** is deliberately deferred — all clients are v1 plaintext,
  fine for local/CI ephemeral containers. Additive behind features + `connect(url, opts)` when
  env-testing against secured clusters is actually needed.
