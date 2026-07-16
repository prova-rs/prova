# Aspirational examples (design showcases — not runnable)

These files illustrate the *intended* authoring surface end to end. They are **documentation, not
runnable tests**, because they use planned API not yet in the engine. The LuaLS stub declares these
(so they type-check), but the runtime doesn't implement them yet:

- `f:use(fixture)` — flow-scoped fixture on the flow builder (today: use `t:use` inside steps).

(Parametrized fixtures via `ctx:param()` were considered and **dropped** — see
`docs/design/north-star-roadmap.md`. `http_service.lua` no longer uses them; parametrization stays
explicit via `test_each` / separate suites / profiles.)

They also reference a live service (`http://localhost:8080` with no server behind it), so they are
illustrative of the execution model rather than runnable against real infrastructure.

The files were deliberately dropped from the `*_test.lua` naming so `prova` discovery skips them and
`examples/*.lua` stays a directory of examples that actually run. When the planned API lands, these
graduate back into runnable examples (paired with real fixtures/containers).

| File | Showcases | Needs |
|------|-----------|-------|
| `ordering.lua` | flow + `depends_on` + resource gating | `f:use`, a live service |
| `dependent_flows.lua` | flow-to-flow DAG (diamond) | `f:use`, a live service |
| `http_service.lua` | render → build → boot → probe, table-driven | a live service |

**Graduated (2026-07-15):** the capstone, kitchen-sink, and suite examples once lived here as
design sketches using `postgres.container` / `mysql.container` / … as built-in globals. The resource
clients moved out of core into external docker-exec plugins (`prova-rs/prova-<name>`), and these
examples graduated to runnable tests using `require("postgres")` + a `prova.toml` declaring the
plugin:

- `../service-grpc-postgres/` — the North Star single-service capstone (plugin)
- `../service_grpc_postgres_primitives_test.lua` — the same capstone via docker primitives (no plugin)
- `../kitchen-sink/` — multi-resource assembly (postgres + mysql + pulsar plugins)
- `../kitchen_sink_primitives_test.lua` — the same topology via primitives (no plugin)
- `../suite/` — a `Scope.Suite` shared-Postgres across files (plugin)
