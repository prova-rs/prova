# Prova — The Authoring Surface

> Companion to [`foundations.md`](foundations.md) (the thesis), [`architecture.md`](architecture.md)
> (the engine), [`manifest.md`](manifest.md) (the package schema), and [`suites.md`](suites.md)
> (multi-file structure). This doc is the **built** authoring surface — everything here runs today.
>
> **The always-current version of this material is the binary itself.** `prova learn authoring`
> renders the one-screen version; `prova.help("<filter>")` (in any test or `prova eval`) and the
> MCP `introspect` tool return every function's `{ name, signature, summary }`, parsed from the
> same LuaCATS stubs that drive editor completion — so they cannot drift from what an author
> sees. When this doc and the binary disagree, the binary is right and this doc has a bug.

## What Prova is

A programmable, language-agnostic **black-box acceptance-test runner**: a real scripting language
(Lua 5.4) plus a real fixture model, shipped as a single static binary. It brings a system into
existence (render it, build it, boot it), then probes it from outside — shell, HTTP/gRPC/GraphQL,
filesystem, containers — with fixtures holding setup and teardown together. Proofs are
`*_test.lua` (or `*.test.lua`) files; a `prova.toml` manifest declares what to run and how.

## Test files, and the globals in them

Everything is a global — no imports except plugins via `require`:

```lua
local postgres = require("postgres")            -- declared in prova.toml [plugins]

local db = prova.fixture("db", Scope.File, function(ctx)
  return postgres.container(ctx)                -- { client, url, container, host, port }
end)

prova.test("rows persist", { requires = { "docker" } }, function(t)
  local d = t:use(db)                           -- lazy DI; built once per scope, torn down LIFO
  d.client:execute("insert into items values (1, 'widget')")
  t:expect(d.client:query_value("select count(*) from items")):equals(1)
end)
```

### Declarators (the `prova` table)

| Call | What it declares |
|---|---|
| `prova.test(name, [opts], fn)` | one test; returns a unit handle (usable in `depends_on`) |
| `prova.test_each(template, cases, fn)` | one test per case; `{key}` placeholders fill the name; the case arrives as the body's 2nd arg and as `t.case` |
| `prova.describe(label, fn)` | a labeling group; **bare** `prova.test`/`group`/`flow` inside the body nest under it |
| `prova.group(name, [opts], fn)` | independent, parallelizable, isolated children — the body receives a **builder** (`g:test`, `g:test_each`, `g:group`, `g:flow`, `g:describe`) |
| `prova.flow(name, [opts], fn)` | **ordered** steps sharing closure state — the body receives a builder (`flow:step(name, [opts], fn)`); once a step fails, later steps cascade-**skip** |
| `prova.fixture(name, [scope,] factory)` | a named, scoped, lazy, cached resource (default `Scope.Test`) |
| `prova.topology(name, [scope,] factory)` | a fixture that is also **name-addressable** by `prova up <name>` / MCP `up` (default `Scope.File`) |

Inside a `group`/`flow` body, a *bare* `prova.test` is an error (it would register at the file
root, outside the unit being built) — children go on the builder. `depends_on` takes **handles**,
not strings; an upstream that failed or skipped **skips** the dependent (never fails it, never
passes state).

### Unit options (`opts` on test/group/flow/step)

`tags = {...}` (select with `--tags a,b`; `!tag` excludes) · `requires = {...}` (capability
gating, below) · `timeout = "60s"` · `depends_on = { handle, ... }` ·
`resources = { prova.port(5432), prova.writes("db"), prova.reads("cache") }` ·
`serial = true` (process-wide exclusive). Group-level `tags`/`requires`/`resources`/`serial`/
`depends_on` are inherited by everything inside.

## Fixtures and scopes

A fixture is **named, scoped, lazy, and cached**; its teardown is registered in the factory and
guaranteed LIFO at scope end. Scopes are **typed values**, not strings:

- `Scope.Test` (default) — fresh per test, torn down after it.
- `Scope.Flow` — one instance across a flow's steps.
- `Scope.File` — one instance per source file.
- `Scope.Suite` — one instance across all files that share a suite's Lua state (see
  [`suites.md`](suites.md)); declared in `suite.lua`.

`t:use(fixture)` resolves lazily at execution: first use builds (async — factories can await),
later uses in the same scope hit the cache. Because caching is scope-keyed, `t:use(f)` inside a
flow's steps returns the *same* instance — that is the flow-fixture story (see the decision
record below for what was deliberately not built).

A `prova.topology` is the same mechanism made name-addressable: `prova up <name>` provisions it
and holds it warm; proofs and `prova eval` reach the held instance through the same `t:use`.
Inside a topology factory, `ctx.network` is an ambient managed docker network.

## The context (`t` / `ctx` — one object)

`t:use(target)` · `t:defer(fn)` (own-scope teardown) · `t:manage(resource)` (auto `stop()`/
`close()` at scope end; returns the resource) · `t:tempdir()` · `t:log(msg)` ·
`t:expect(value, label?)` · `t:expect_all(fn)` (soft: collect several failures) ·
`t:skip(reason)` · `t.case` (the `test_each` case).

## Assertions

`t:expect(v)` returns a matcher: `equals` (deep) · `is` · `is_true` · `is_truthy` · `is_falsy` ·
`is_nil` · `contains` · `matches` · `has_length` · `is_one_of` · `gt gte lt lte` · `exists` ·
`is_file` · `is_dir` · `is_empty` · `is_fully_rendered` · `matches_snapshot`. Negate with
`:never()`. Failures carry `expected X, got Y` detail plus the test's `file:line` in every
reporter.

- **Soft assertions**: `t:expect_all(function(e) e:expect(a):equals(1); e:expect(b):equals(2) end)`
  — all failures collect into one message.
- **Snapshots**: `t:expect(value_or_tree):matches_snapshot([key], { level = "layout"|"content" })`.
  `.snap` files live beside the test file (`snapshots/`); `-u/--update-snapshots` rewrites;
  `--unreferenced warn|delete` reconciles orphans on **full** runs only (a filtered run would
  make unrun tests' snapshots look orphaned, so the check skips with a note).

## Async, timing, readiness

Bodies are async-driven; I/O awaits cooperatively. `prova.sleep(ms)` exists but readiness is
never a sleep: `prova.retry(fn, { timeout = "30s", every = "500ms" })`, `http.wait_for(url,
{ status, timeout })`, `grpc.wait_for(addr)` — gate on a condition that HOLDS. `timeout` on a
unit cancels its future at the deadline (`timed out after …`); teardown still runs.

## Capability gating: `requires` (skip) vs `must_run` (fail)

`requires = { "docker", "dotnet >= 9" }` states a **fact about the test** — what it needs. The
vocabulary is open: a name is a binary probed on `PATH` (special cases: `docker` probes the live
daemon; `github` checks `GITHUB_TOKEN`; native names like `http`/`grpc` check compiled
features), and `runtime.capability(name, fn)` in the companion registers package-specific
predicates. Unavailable ⇒ the node **skips with the reason shown, never fails** — which also
means a typo'd name silently skips: read skip reasons. A profile's `must_run = [...]` is the
other half — **policy about the environment**: same expressions, but unmet ⇒ the run FAILS
(exit 2) before anything executes. See [`test-topology.md`](test-topology.md).

## Modules (built-ins) and plugins

Built-ins, one line each: `shell.run(cmd, {cwd, env, timeout, check})` + `shell.spawn` (managed
process) · `fs` (read/write/exists/glob/tempdir/remove_all) · `net.free_port()` ·
`http.get/post/…` (async; `.status`, `:json()`), `http.client`, `http.wait_for` ·
`grpc.client(addr)` (`:call`, `:call_status`; server reflection), `grpc.wait_for` ·
`graphql.client{ url }` · `yaml.parse/parse_all` · `sqlite.client(url)` ·
`docker.run{...}`/`build`/`network` · mock facets: `http.mock`/`grpc.mock` (stubs with Lua reply
handlers + a request journal; see [`mocks-proxies-drivers.md`](mocks-proxies-drivers.md)) —
plus `archetect.render{...}`/`verify{...}`, a bundled plugin (always present in the standalone
binary).

**There is no `db` module.** Server databases are plugins — `require("postgres")`,
`require("mysql")` — following the resource grammar: `X.client(...)` attaches to something
running; `X.container(ctx, opts?)` provisions ephemerally and returns
`{ client, url, container, host, port }` (see [`namespacing.md`](namespacing.md)). The full,
current surface — core and every plugin this package declares — is one call away:
`prova.help()` / MCP `introspect`.

## Files that aren't tests

- **`suite.lua`** — in a directory, groups that directory's test files into one suite sharing a
  Lua state (`Scope.Suite` fixtures live here; `suite.config{ name?, requires? }` names/gates
  the whole suite). Directory-scoped: subdirectories are discovered independently.
- **`prova.lua`** (or the manifest's `config` path) — the package **companion**, loaded once
  with the manifest, *before* any suite: `runtime.capability(name, fn)` registrations live
  here. It is NOT a conftest — shared fixtures belong in `suite.lua`; shared helpers in a
  `require`d plugin under `plugin_root`.

## Running (pointer)

The run path is bare **`prova`** (`prova <file-or-dir>…` bypasses the manifest); there is no
`test` verb. Selection: `-k PATTERN` / `--tags a,b` / `--node PATH` / `--last-failed` (an
explicit selection matching nothing is an error; `--allow-empty` opts out). Output: `--format
console|json|tap`, `--color`, `-q`, `--junit PATH`, `--gha` — the console is a streaming tree
with a `failures:` recap, and every event carries the test's `file:line`. `--jobs` = concurrent
**suites**, throughput only. Full schema and precedence: [`manifest.md`](manifest.md); live
selection doctrine: `prova learn running`.

## Decision record — deliberate absences

These are not gaps; they were assessed and **dropped** (2026-07-15, phase-1 ergonomics):

- **Parametrized fixtures** (`ctx:param()`, `{ params = ... }`): a parametrized fixture silently
  multiplies the tests that use it — action-at-a-distance, pytest's most-confusing feature —
  and the lazy `t:use` model has no static fixture graph to do it cleanly anyway. The need
  decomposes: same assertions over data → `test_each`; divergent logic → separate files;
  env-level variation → profiles.
- **`f:use` (flow-level fixture binding)**: the flow *builder* runs at collection; fixture
  *values* exist only at execution, so `f:use` could only be a lying proxy or a re-run builder —
  and the only principled consumer of re-runnable builders is a load executor, an explicit
  non-goal. Flow fixtures are `t:use` inside steps (scope-cached ⇒ same instance).
- **`before_all`/`before_each`/`after_each`/`after_all` and autouse fixtures**: hooks reorder
  action at a distance; prova's setup is explicit (`t:use`) and its teardown is owned by the
  thing that created it (`defer`/`manage`, factory-registered). Nothing implicit runs around a
  test.
- **`describe_each`**: not built until a real trigger appears (the same case-list copied across
  several `test_each`, or a whole block × N variants). It composes `describe` + `test_each`,
  both shipped — a cheap add when the need is real.
- **A params DSL**: parametrize with plain Lua — a `VARIANTS` table and a `for` loop generating
  fixtures + groups is the idiom.
