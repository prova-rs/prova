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

## Decision (2026-07-15): `f:use` — DROPPED

Assessed and dropped, same reasoning as `ctx:param`: it's magic that fights the explicit,
lazy-`ctx:use` model. The flow *builder* runs at collection; a fixture *value* only exists at
execution, so `f:use` could only work via (a) a transparent proxy that lies to `type()` and breaks
when passed bare to native functions — the exact footgun class we rejected — or (b) re-running the
flow builder per execution.

**The re-runnable-flow-builder assessment (the crux):** its only substantial consumer is a **load
executor**, and load testing is an explicit **non-goal** (`foundations.md`: "stays with k6/Gatling…
measure timing, not model load"). Test and step bodies are *already* re-runnable (`run_one` builds a
fresh `t` per call — principle #2); only the structural flow *builder* runs once, which is correct and
consistent with every other declarator (`group`, `describe`). With no principled consumer, re-running
the builder is speculative infrastructure — dismissed.

**Resolution:** flow-scoped fixtures use `t:use` inside steps. Because fixtures are scope-cached,
`t:use(f)` returns the *same* instance across a flow's steps — identical semantics to what `f:use`
promised, via the one fixture mechanism prova already has. `ordering.lua` and `dependent_flows.lua`
rewritten accordingly; `FlowBuilder:use` removed from the LuaLS stub.

## Phase 1 ergonomics — RESOLVED

Both remaining "features" dropped; the parametrization + fixture story is complete and explicit:

| Need | Construct | Status |
|---|---|---|
| data-driven tests (shared assertions) | `test_each` | ✅ shipped |
| a whole *block* ×N (shared assertions) | `describe_each` | not built — trigger documented (below) |
| divergent variants (SQL vs document) | separate suites/files | ✅ |
| env-level variation | profiles / `prova.toml` | ✅ |
| flow-scoped fixtures | `t:use` inside steps (scope-cached) | ✅ |

### `describe_each` trigger (so it isn't lost)

Build it *only* when one of these appears — until then it's speculative:
- the same case-list copied across several `test_each` in one file, or
- copy-pasting a whole suite/file to change one variant constant, or
- wanting to run an existing `describe` block over N shared-assertion variants.

It composes `describe` (parent_stack) + `test_each` (case-threading), both built — a cheap additive add
when the need is real.

## Graduation targets (now unblocked on authoring)

`ordering.lua`, `dependent_flows.lua`, `http_service.lua` no longer wait on any authoring feature — they
need only a **live service backend**, so they graduate alongside the Phase 2 capstone (a real or
`shell.spawn`ed service to run them against).
