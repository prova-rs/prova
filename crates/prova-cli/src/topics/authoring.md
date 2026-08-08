# authoring — the proof DSL, one screen

Files match `*.prova.lua` — named for the collector, since a file may declare tests, fixtures, topologies, and reminders (`*_test.lua` / `*.test.lua` are the accepted older spellings; bare `prova.lua` is the manifest companion, never collected). Everything is a global — no imports except packages via `require`.

```lua
local postgres = require("postgres")            -- declared in prova.toml [dependencies]

local db = prova.fixture("db", Scope.File, function(ctx)
  return postgres.container(ctx)                -- teardown rides the scope, LIFO, guaranteed
end)

prova.test("rows persist", { requires = { "docker" } }, function(t)
  local d = t:use(db)                           -- lazy: built on first use, cached per scope
  d.client:execute("insert into items values (1, 'widget')")
  t:expect(d.client:query_value("select count(*) from items"), "count"):equals(1)
end)
```

## The vocabulary

- `prova.test(name, [opts], fn)` · `prova.test_each(name_template, cases, fn)` ·
  `prova.describe(label, body)` (labels only).
- `prova.group(name, [opts], body)` — independent, parallel, isolated.
  `prova.flow(name, [opts], body)` — ordered steps sharing state; a failed step
  cascade-skips the rest. **Both bodies receive a BUILDER** — declare children on it
  (`function(g) g:test(...) end` · `function(flow) flow:step("...", fn) end`); a bare
  `prova.test` inside either body is an error, not a child.
- Cross-unit gating: `depends_on = { handle }` — handles, not strings. Upstream failure SKIPS
  downstream, never fails it, never passes state.
- opts: `tags`, `requires`, `timeout = "60s"`, `serial = true`,
  `resources = { prova.port(N), prova.writes("db"), prova.reads("cache") }` — name the ACCESS MODE:
  `prova.writes(x)` is an exclusive hold, `prova.reads(x)` a concurrent one, and either accepts a
  bare token or a ref the other made (`prova.reads(prova.port(5432))`). A bare string and
  `prova.port` are writers by default. Groups' `tags`/`requires` are inherited.
  Tests and flows also take `promises = "reason"` — a proof authored ahead of its implementation
  (`prova learn promises`); never on a group or in `suite.config`.
- Matchers on `t:expect(v, label?)` — negate any with `:never()`. Grouped by what the SUBJECT is,
  because a flat list hides which ones ask the filesystem:
  - **any value**: `equals is is_true is_falsy is_nil contains matches has_length is_one_of exists`
  - **numbers**: `gt gte lt lte`
  - **paths** (a path string, or a handle carrying `path`): `is_file is_dir is_fully_rendered`,
    and `exists`/`is_empty` when the subject IS path-shaped
  - `is_empty` and `exists` are polymorphic: empty/present *for whatever the subject is*. A
    **string** is resolved as a path, since asserting a file is there is the load-bearing use — so
    `expect("some value"):exists()` asks the filesystem. For a string's presence use
    `never():is_nil()`.

  `t:expect_all(fn)` collects soft failures; `t:skip(why)`.
- `requires = { "docker", "dotnet >= 9" }`: a capability is a tool name checked on PATH
  (`docker` probes the daemon; version constraints compare). Missing → the node SKIPS with the
  reason shown, never fails — so a TYPO'D NAME SILENTLY SKIPS; read skip reasons. Custom
  predicates: `runtime.capability(name, fn)` in the `prova.lua` companion.
- Snapshots: `t:expect(tree):matches_snapshot{ level = "layout"|"content" }`; `-u` rewrites;
  review `.snap` diffs like code; `--unreferenced warn` catches orphans in CI.
- There are NO before_each/after_each hooks — a fixture is the setup that produces a value;
  `ctx:manage(resource)` / `ctx:defer(fn)` are the teardown that belongs to one. See
  `prova learn fixtures`.
- Parametrize with plain Lua: a `VARIANTS` table + a `for` loop generating fixtures and groups.
  There is deliberately no params DSL.

## Readiness, never sleep

```lua
shell.run("cargo build", { cwd = dir, timeout = "600s", check = true })
local port = net.free_port()
local proc = ctx:manage(shell.spawn(bin, { env = { PORT = port } }))
http.wait_for("http://127.0.0.1:" .. port .. "/health", { timeout = "60s" })
-- readiness failed? proc:output() holds the app's last 64KB of combined output
```

Assert effects where they land: probe the API AND cross-check the database.

Go deeper: `prova learn fixtures` (scopes) · `prova learn doubles` (dependencies) ·
`prova learn running` (selection). Shapes: `prova.help("<name>")`.
