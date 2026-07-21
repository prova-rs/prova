# authoring — the proof DSL, one screen

Files match `*_test.lua`. Everything is a global — no imports except plugins via `require`.

```lua
local postgres = require("postgres")            -- declared in prova.toml [plugins]

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

- `prova.test(name, [opts], fn)` · `prova.test_each(cases, name, [opts], fn)` ·
  `prova.describe(label, body)` (labels only).
- `prova.group(name, [opts], body)` — independent, parallel, isolated.
  `prova.flow(name, [opts], body)` — ordered steps sharing state; a failed step
  cascade-skips the rest.
- Cross-unit gating: `depends_on = { handle }` — handles, not strings. Upstream failure SKIPS
  downstream, never fails it, never passes state.
- opts: `tags`, `requires`, `timeout = "60s"`, `serial = true`,
  `resources = { prova.port(N), prova.shared("db") }`. Groups' `tags`/`requires` are inherited.
- Matchers on `t:expect(v, label?)`: `equals is is_true is_falsy is_nil contains matches
  has_length is_one_of gt gte lt lte exists is_file is_dir is_empty is_fully_rendered
  matches_snapshot` — negate with `:never()`. `t:expect_all(fn)` collects soft failures;
  `t:skip(why)`.
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
