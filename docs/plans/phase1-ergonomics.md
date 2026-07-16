# Plan: finish Phase 1 authoring ergonomics

Design refs: [`docs/design/api.md`](../design/api.md),
[`docs/design/north-star-roadmap.md`](../design/north-star-roadmap.md) §Phase 1.2.

## Decision (2026-07-15): parametrized fixtures — DROPPED

`ctx:param()` + `{ params = {...} }` are cut, not built. Rationale:

- **They fight prova's explicit model.** A parametrized fixture silently *multiplies* the tests that
  transitively use it — pytest's most-confusing feature (action-at-a-distance). Prova's parametrization
  is deliberately explicit (`test_each`, `describe`, profiles).
- **The lazy `ctx:use` model can't do the clean version anyway.** Usage-driven multiplication needs a
  static fixture-dependency graph prova doesn't have (fixtures resolve lazily inside bodies). The only
  implementable variant is scope-driven ("a Suite-param fixture parametrizes the whole file"), which is
  still action-at-a-distance. The architecture is steering us away from a footgun — take the hint.
- **The real need decomposes without it,** by whether the *assertions* are shared:
  | Variation | Shared assertions? | Construct | Status |
  |---|---|---|---|
  | same test, varying data | yes | `test_each` | ✅ |
  | divergent logic (SQL vs document store) | no | separate suites/files | ✅ |
  | env-level (local/CI/cluster) | n/a | profiles / `prova.toml` | ✅ |
  | a whole *block* ×N, shared assertions | yes | `describe_each` | not built (add only on real need) |

Removed the reserved surface: `Context:param()` and `FixtureOpts.params` in `library/prova.lua`; the
roadmap bullet. `t.case` (from `test_each`) stays — that's the explicit, visible form.

## Remaining work — `f:use(fixture)` on the flow builder

The one real Phase 1 gap. Target API (`examples/aspirational/ordering.lua`):

```lua
prova.flow("order lifecycle", { ... }, function(f)
  local base = f:use(api)          -- resolve a fixture once for the whole flow
  f:step("create", function(t) http.post(base .. "/orders", ...) end)
  f:step("read",   function(t) http.get(base .. "/orders/" .. id) end)
end)
```

### The tension

The flow-builder body runs at **collection**; `f:use(api)` wants a fixture *value* to close over, but
fixtures resolve at **execution** (inside a step). Today this works via `t:use(api)` inside each step.

### Options

1. **Deferred-resolution proxy** — `f:use` returns a proxy resolved on first step. Fragile: `base .. x`
   and `base.field` need `__concat`/`__index` metamethods; leaks abstraction.
2. **Pre-step resolution + explicit deref** — `f:use` records the request on the flow; before each step
   the engine resolves it once (cached in flow scope) and the step reads the value via a handle.
3. **Rewrite examples to `t:use`** — no engine change; `f:use` stays unbuilt.

Decision pending until implementation starts — evaluate 1 vs 2 against the real flow execution path
(where step bodies get their `Ctx`, and whether flow scope is already threaded there).

## Graduation targets

- `ordering.lua`, `dependent_flows.lua` → runnable once `f:use` lands (+ a live service backend; may
  stub with a local archetype / a `shell.spawn`ed toy service as `rust_cli` did).
- `http_service.lua` → rewrite to explicit `test_each`/`describe` (no longer blocked on `ctx:param`);
  still needs a live service backend.

## Verify every step

`cargo test`, `cargo clippy --all-targets` (zero warnings), `lua-language-server --check`, and run the
touched `examples/*.lua` via the CLI. Keep the LuaCATS stub (`library/`) in lockstep with the runtime.
