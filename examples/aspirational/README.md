# Aspirational examples (design showcases — not runnable)

These files illustrate the *intended* authoring surface end to end. They are **documentation, not
runnable tests** — not because of any missing authoring feature (there are none left: both `ctx:param`
and `f:use` were assessed and **dropped** as magic that fights prova's explicit model — see
`docs/design/north-star-roadmap.md`), but because they reference a **live service**
(`http://localhost:8080` with no server behind it). They use only shipped API now (fixtures via
`t:use`, `test_each`, `describe`), so they illustrate the real execution model.

The files were deliberately dropped from the `*_test.lua` naming so `prova` discovery skips them and
`examples/*.lua` stays a directory of examples that actually run. Once paired with a real (or
`shell.spawn`ed) service backend — with the Phase 2 capstone — they graduate to runnable tests.

| File | Showcases | Needs |
|------|-----------|-------|
| `dependent_flows.lua` | flow-to-flow DAG (diamond) | a live service — **can graduate**, same as `ordering` did |
| `http_service.lua` | render → build → boot → probe, table-driven | a **real** service: archetect + cargo (Phase 2 capstone) |

**A distinction worth keeping straight**, now that `http.mock` exists: "needs a live service" meant
two different things in this table, and only one of them is a mock's business.

- In `ordering.lua`/`dependent_flows.lua` the service is **scaffolding** — the *primitives* (flows,
  DAG edges, resource gating) are what's on show, and the API is just something to order calls
  against. A stateful `http.mock` is a legitimate stand-in, and that is exactly how `ordering`
  graduated.
- In `http_service.lua` the service **is the system under test** — the whole point is render → build
  → boot → probe. Standing a mock in for it would be mocking the SUT, which is a non-goal
  (`docs/plans/mocks.md`): the test would pass while proving nothing. It waits on the Phase 2
  capstone, exactly as it always did.

**Graduated (2026-07-16):** `ordering.lua` → [`../ordering_test.lua`](../ordering_test.lua), against a
stateful `http.mock` — no docker, no network, no build. It doubles as the worked example of a
**stateful fake**: a `:reply` handler is real Lua, so the fake's state is an ordinary table the
fixture closes over and the ordinary matchers assert on
(`t:expect(svc.orders[id].status):equals("cancelled")`). No state API required.

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
