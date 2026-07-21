# fixtures — scoped provisioning: setup and teardown are one thing

A fixture is a named, scoped, lazy, cached value with guaranteed LIFO teardown. It is the whole
setup/teardown model — there are no xunit hooks.

```lua
local db = prova.fixture("db", Scope.File, function(ctx)
  local d = require("postgres").container(ctx)   -- ctx ties teardown to the fixture's scope
  d.client:execute("create table items (id int)")
  return d
end)

prova.test("uses it", function(t)
  local d = t:use(db)        -- built HERE on first use; same instance for the whole scope
end)
```

## Scopes — how long the value lives

| Scope | Built once per | Reach for it when |
|---|---|---|
| `Scope.Test` (default) | test | isolation matters more than speed |
| `Scope.Flow` | flow | ordered steps share one provisioned thing |
| `Scope.File` | file | the file's tests share an expensive resource |
| `Scope.Suite` | suite (files sharing one Lua state) | cross-file sharing — see `[suites]` / a dir's `suite.lua` |

Lazy: never built if nothing `use`s it (deselected tests provision nothing). Cached: one build
per scope, everyone gets the same instance. Teardown at scope end, LIFO, guaranteed — even on
failure.

## Context — the handle that makes teardown automatic

- `ctx:use(handle_or_name)` — dependency-inject another fixture (fixtures can use fixtures).
- `ctx:manage(resource)` — anything with `:stop()`/`:close()` is torn down at scope end.
- `ctx:defer(fn)` — arbitrary teardown, LIFO with the rest.
- `ctx:tempdir()` — a scope-owned scratch dir. Bigger shape: `require("prova.workspace")`.
- In tests the context is `t` (same object: `t:use`, `t:expect`, `t:skip`).

## Rules that bite

- Return the value the test needs (client + url + handle), not booleans.
- Never tear down manually in the test body — that is what the scope is for.
- A fixture that only performs an action (no value) is a smell: make the action part of the
  fixture that owns the resource it acts on.
- Suite = one Lua state = the parallelism unit; `--jobs` can never change what a run means.

Go deeper: `prova learn topologies` (the fixture that outlives a run) · `prova learn doubles`.
