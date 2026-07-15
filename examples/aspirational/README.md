# Aspirational examples (design showcases — not runnable)

These files illustrate the *intended* authoring surface end to end. They are **documentation, not
runnable tests**, for two reasons:

1. **They target things that aren't here.** They reference live services (`http://localhost:8080`
   with no server behind it) and remote archetypes that must be rendered + `cargo build`-ed over the
   network.
2. **They use planned API not yet in the engine.** The LuaLS stub declares these (so they
   type-check), but the runtime doesn't implement them yet:
   - `f:use(fixture)` — flow-scoped fixture on the flow builder (today: use `t:use` inside steps).
   - parametrized fixtures via `ctx:param()` + `{ params = { ... } }`.

   (`prova.test_each` and `prova.describe` have **landed** — `rust_cli.lua` graduated to
   `examples/rust_cli_test.lua`, rendering a local Lua archetype and building it offline.)

The files were deliberately dropped from the `*_test.lua` naming so `prova` discovery skips them and
`examples/*.lua` stays a directory of examples that actually run. When the planned API lands, these
graduate back into runnable examples (paired with real fixtures/containers).

| File | Showcases | Needs |
|------|-----------|-------|
| `ordering.lua` | flow + `depends_on` + resource gating | `f:use`, a live service |
| `dependent_flows.lua` | flow-to-flow DAG (diamond) | `f:use`, a live service |
| `http_service.lua` | render → build → boot → probe, table-driven | `ctx:param`, a live service |
| `service_grpc_postgres.lua` | the North Star single-service capstone | `require("postgres")` + a `prova.toml` plugin decl; a rendered+built archetype |
| `service_grpc_postgres_primitives.lua` | the capstone via docker primitives | same |
| `kitchen_sink.lua` | multi-resource assembly | the resource plugins declared in `prova.toml` |
| `kitchen_sink_primitives.lua` | multi-resource via primitives | same |
| `suite/` | a `Scope.Suite` shared-Postgres across files | `require("postgres")` + a `prova.toml` plugin decl |

**The resource clients moved out of core (2026-07-15):** databases/caches/brokers/etc. are now
external docker-exec plugins (`prova-rs/prova-<name>`), so the capstone/kitchen/suite examples above —
which used `postgres.container` / `kafka.container` / … as built-in globals — need updating to
`require("postgres")` + a `prova.toml` declaring the plugin. They graduate back to runnable
`*_test.lua` once the plugins are published and the declarations added.
