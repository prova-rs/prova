# Suites — a comprehensive, elegant, flexible grouping model

> A **suite** is a named group of test files that share **one Lua state** — the unit of shared setup
> (suite-scoped fixtures), isolation, and parallelism. This doc defines the model, the semantics, and
> the implementation plan. Companion to [`architecture.md`](architecture.md).

## The core insight (why this shape)

prova runs each file in its own `mlua::Lua` VM because `Lua` and the collected `Function` bodies are
`!Send` — they can't cross threads. That made the **file** the parallelism boundary, and it made a
cross-file `Scope.Suite` fixture impossible: a live Lua value (a `db.Connection`, a running container
handle) can't be shared between two VMs.

Rather than fight `!Send` by serializing suite fixtures across per-file states, **move the boundary up
from file to suite:**

- A **suite = one Lua state = one scheduling/parallelism unit.**
- Within a suite, all the suite's files load into **that one state**. `Scope.Suite` fixtures are then
  just ordinary cached **live values** — the container stays up, the connection stays open — shared
  across every file in the suite with **zero serialization**. `Scope.File` rebuilds per file,
  `Scope.Test` per test, `Scope.Flow` per flow — all still meaningful.
- **Suites run in parallel** across worker threads, each with its own state — naturally isolated (a
  file in suite `grpc` never sees suite `rest`'s Postgres).

This turns the `!Send` constraint from a limitation into the design: isolation and shared-state fall
out of "one state per suite," and multi-core parallelism moves to the suite grain — which is the
*right* grain (you have many suites; a giant single suite that must share state genuinely can't be
cross-core anyway).

## Backward compatibility: every file is a singleton suite

**A file not assigned to any declared suite is its own one-file suite.** That preserves today's
behaviour exactly — files parallelize, and within a one-file suite `Scope.Suite` == `Scope.File`
(which is *correct*, not a lie: the suite *is* the file). Nothing existing changes.

## Defining suites — three layers (elegant defaults, explicit control)

1. **`suite.lua` convention (zero config).** A `suite.lua` in a directory declares a suite whose
   members are its sibling `*_test.lua` files (recursively, until a nested `suite.lua`). `suite.lua`
   runs **once, first, in the suite's state** — it's where suite-scoped fixtures and shared config
   live, colocated with the files that use them. This is the blessed, discoverable form.

2. **`prova.toml` manifest (explicit / cross-cutting).** For grouping that doesn't match the directory
   tree, or per-suite `jobs`/`env`/`requires`:
   ```toml
   [suites.grpc]
   paths = ["services/grpc/**/*_test.lua"]
   setup = "services/grpc/suite.lua"   # optional
   requires = ["docker"]
   env = { RUST_LOG = "info" }
   ```

3. **Implicit singletons.** Everything else — each ungrouped `*_test.lua` is a one-file suite.

## The `suite.lua` setup file

Runs once in the suite state, before any test file. It declares suite fixtures with the **same**
`prova.fixture(name, Scope.Suite, factory)` API and (optionally) suite config:

```lua
-- services/grpc/suite.lua  — runs once for the whole grpc suite
suite.config{ name = "grpc", requires = { "docker" } }

-- One Postgres for every file in the suite; the container stays up for the suite's lifetime.
prova.fixture("pg", Scope.Suite, function(ctx)
  return db.postgres(ctx, { database = "orders" }).conn   -- a live connection, cached in the suite state
end)
```

Test files reference suite fixtures **by name** (the already-supported string form of `ctx:use`):

```lua
-- services/grpc/create_test.lua
prova.test("create persists a row", function(t)
  local pg = t:use("pg")            -- the suite's shared connection (built once, reused)
  ...
end)
```

Name-based reference is the clean cross-file contract: `suite.lua` *defines* named suite fixtures; test
files *consume* them by name. (Handles are per-state Lua values, so a handle can't be imported across
chunk boundaries — but a name can, and the value is one shared instance in the suite state.)

## Scope semantics inside a suite

| Scope | Lifetime within a suite |
|---|---|
| `Scope.Test` | rebuilt per test |
| `Scope.Flow` | once per flow, shared across its steps |
| `Scope.File` | once per file — the runner resets the file-scope cache at each file boundary |
| `Scope.Suite` | once for the whole suite — a live cached value in the suite state; torn down once at suite end |

`Scope.File` staying correct in a multi-file suite is the one piece that needs runner support: test
nodes are tagged with their source file, and the file-scope `ScopeState` is reset when the running
file changes.

## Parallelism & lifecycle

- **`--jobs` = number of concurrent *suites*** (was: concurrent files). Within a suite, the async
  scheduler still overlaps I/O-bound tests cooperatively on one core; across suites is true multi-core.
- **Teardown** runs once per suite: `Scope.Suite` teardown (containers stopped, connections closed)
  fires after the suite's last test, in the suite state — no leaks, no double-provisioning.
- A suite that `requires` an unmet capability skips **all** its files (cascade), reported once.

## Status (2026-07-13)

**Built and tested.** A file-index is threaded through `Node → PlanItem → Ctx` (per-file `Scope.File`);
`run_suite_files` loads a suite's setup + members into one state and runs a combined plan with one
suite teardown; `discover_suites` groups by the `suite.lua` convention (+ singletons); the CLI runs
suites. `suite.config{ name, requires }` gates a whole suite. `examples/suite/` provisions ONE Postgres
in a `Scope.Suite` fixture and shares it across two files (`b_read_test` sees the row `a_create_test`
inserted) — verified real, one container, torn down once. Remaining: manifest `[suites.*]` (below, #5).

## Implementation plan (incremental)

1. **Suite model + discovery.** A `Suite { name, files: Vec<PathBuf>, setup: Option<PathBuf> }`.
   `discover_suites(root)`: walk the tree; a directory with a `suite.lua` becomes a suite owning its
   subtree's `*_test.lua`; everything else → singleton suites. (Manifest `[suites.*]` layers on later.)
2. **Suite runner (the core).** `run_suite_unit(suite)`: one Lua state; load `setup` (if any) then
   every member file into it (accumulating their collectors into one plan); run the combined plan with
   per-file `Scope.File` reset; one suite teardown. Node paths are prefixed with the file (and suite)
   for reporting.
3. **Parallelism.** The worker pool drains a queue of **suites** (not files); a singleton suite is the
   trivial case, so this generalizes `run_pooled`/`run_sequential` with no behaviour change for
   ungrouped files.
4. **`suite.config{...}`** — a global installed in the suite state to set name/jobs/env/requires.
5. **Manifest `[suites.*]`** — explicit grouping + per-suite settings.

Step 1–3 are the substance and deliver working cross-file `Scope.Suite`; 4–5 are additive polish.
