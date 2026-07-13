# Aspirational examples (design showcases — not runnable)

These files illustrate the *intended* authoring surface end to end. They are **documentation, not
runnable tests**, for two reasons:

1. **They target things that aren't here.** They reference live services (`http://localhost:8080`
   with no server behind it) and remote archetypes that must be rendered + `cargo build`-ed over the
   network.
2. **They use planned API not yet in the engine.** The LuaLS stub declares these (so they
   type-check), but the runtime doesn't implement them yet:
   - `f:use(fixture)` — flow-scoped fixture on the flow builder (today: use `t:use` inside steps).
   - `prova.describe(label, fn)` — ambient labeling group (today: `prova.group` with a builder).
   - `prova.test_each(name, cases, fn)` — table-driven parametrized tests.
   - parametrized fixtures via `ctx:param()` + `{ params = { ... } }`.

The files were deliberately dropped from the `*_test.lua` naming so `prova` discovery skips them and
`examples/*.lua` stays a directory of examples that actually run. When the planned API lands, these
graduate back into runnable examples (paired with real fixtures/containers).

| File | Showcases | Needs |
|------|-----------|-------|
| `ordering.lua` | flow + `depends_on` + resource gating | `f:use`, a live service |
| `dependent_flows.lua` | flow-to-flow DAG (diamond) | `f:use`, a live service |
| `rust_cli.lua` | render → assert layout → build | `prova.describe`, network + cargo |
| `http_service.lua` | render → build → boot → probe, table-driven | `test_each`, `ctx:param`, network |
