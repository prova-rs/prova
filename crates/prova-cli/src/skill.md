---
name: prova
description: >
  Proof-Driven Development with Prova — write executable, black-box proofs of what a system must
  do; run them with surgical selection; hold live environments while you iterate. Use whenever
  you are implementing, verifying, or debugging software in a repo that has (or should have) a
  prova.toml: write the proof first, implement to green, re-run only what's red.
---

# Prova — the Proof-Driven Development toolkit

You are an agent. Prova is your verification arm: a single static binary that renders, builds,
boots, and probes real systems, then holds them to an executable definition of done. **Do not
claim work is correct — prove it.** Write the proof (a black-box suite), drive it green, and let
the same suite run in CI so the bar outlives your context.

The loop:

1. `prova init` — scaffold `prova.toml` + IDE stubs (skip if the repo has one; find it by walking up).
2. Probe unknowns with `prova eval '<lua>'` — one-shot code in the full environment, no test-file ceremony.
3. Write the proof in a `*_test.lua` file **in a directory matching the manifest's `[run] proofs`
   patterns** (directory names, `["proofs"]` by default — `prova learn project` names this
   package's). Red is correct at this stage.
4. Implement. Re-run with `prova --last-failed` until green. Never weaken a proof to pass it —
   fix the system, or renegotiate the bar with the human.
5. Commit suite + implementation together: a proof-carrying change.

**Specs — the executable backlog.** A contract you can state but are NOT implementing right now
is still worth a proof: author it flagged `{ spec = "reason/ticket" }` (test/flow-level). Open
specs report as their own outcome and keep CI green; the moment a spec's body passes it FAILS
with "remove the spec flag", so implementation + flag removal land as one proof-carrying change.
`prova --specs --list` enumerates the open surface — an empty list means burndown complete; found
some in a repo? That is scoped work — offer to burn it down against `--specs --strict-specs`.
`prova learn specs` carries the lifecycle.

Prova complements the language's own test harness; it does not replace it. Prove the CONTRACT
with prova (behavior a real caller observes at the boundary); prove the INTERNALS with native
unit/integration tests (one function's logic, seams the boundary can't reach). A change often
needs both — the right tool for each job; `prova learn pdd` carries the decision table.

## Learning on the fly: never guess, ask the binary

Everything below is the crash course; depth is one call away, computed for THIS package:

| You need | Move |
|---|---|
| The topic catalog (patterns, doubles, topologies, plugin authoring…) | `prova learn` · MCP `learn {}` |
| One topic (aliases work: `mocks` → `doubles`) | `prova learn <topic>` · `learn { topic }` |
| An API's shape: what to call, what comes back | `prova.help("<filter>")` in any test/eval · MCP `introspect { filter }` |
| Which archetypes `init` can scaffold | `prova init --list` (or `prova learn init`) |
| A live value's shape | probe it with `eval` |
| The open-spec backlog (proofs ahead of implementation) | `prova --specs --list` · `prova learn specs` |

## Test files, in one screen

Files match `*_test.lua` (or `*.test.lua`). Everything is a global — no imports except plugins.

```lua
local postgres = require("postgres")          -- plugin, declared in prova.toml [plugins]

-- Fixtures: named, scoped, lazy, cached; teardown is guaranteed and LIFO.
-- Scopes: Scope.Test (default) | Scope.Flow | Scope.File | Scope.Suite
local db = prova.fixture("db", Scope.File, function(ctx)
  return postgres.container(ctx)              -- { client, url, container, host, port }
end)

prova.test("rows persist", { requires = { "docker" } }, function(t)
  local d = t:use(db)                         -- dependency injection; builds once per scope
  d.client:execute("insert into items values (1, 'widget')")
  t:expect(d.client:query_value("select count(*) from items")):equals(1)
end)
```

- `prova.test(name, [opts], fn)` / `prova.test_each(name_template, cases, fn)` /
  `prova.describe` for labeling.
- `prova.group` = independent, parallel, isolated. `prova.flow` = ordered steps sharing state,
  cascade-skip on failure. Both bodies receive a BUILDER — children go on it (`g:test(...)`,
  `flow:step(...)`); a bare `prova.test` inside either body is an error. Cross-unit gating: `depends_on = { handle }` (handles, not strings) —
  upstream failure **skips** downstream, never fails it, never passes state.
- opts: `tags`, `requires`, `timeout = "60s"`,
  `resources = { prova.port(N), prova.shared("db") }`, `serial = true`, `spec = "reason"` (a
  proof authored ahead of its implementation — `prova learn specs`). `--jobs` is throughput
  only — it can never change what a run means.
- Context: `ctx:use(handle)`, `ctx:manage(resource)` (auto stop/close at scope end),
  `ctx:defer(fn)`, `ctx:tempdir()`, `t:expect(v, label?)`, `t:expect_all(fn)` (soft), `t:skip(why)`.
- Matchers: `equals is is_true is_falsy is_nil contains matches has_length is_one_of gt gte lt
  lte exists is_file is_dir is_empty is_fully_rendered matches_snapshot` — negate with `:never()`.
- `requires = { "docker", "cargo", ... }`: a capability is a **tool name checked on `PATH`**
  (special cases: `docker` probes the live daemon; `github` checks `GITHUB_TOKEN`; native names
  like `http`/`grpc` check compiled features). Missing ⇒ the node **skips with the reason shown,
  never fails** — which also means a TYPO'd name silently skips; read skip reasons in the output.
  Groups' `requires`/`tags` are inherited by everything inside them.
- Snapshots: `t:expect(tree):matches_snapshot{ level = "layout"|"content" }`; `-u` rewrites;
  review `.snap` diffs like code.
- Parametrize with plain Lua — a `VARIANTS` table and a `for` loop generating fixtures + groups
  per variant is the idiom (there is deliberately no params DSL).

## Resources: the grammar

Every service resource — plugin or hand-rolled — is the same shape: **`X.client(...)` attaches to
something running; `X.container(ctx, opts?)` provisions ephemerally and returns
`{ client, url, container, host, port }`**. `url` is what you inject into the app under test;
`host`/`port` split it for discrete env vars. Declare plugins in `prova.toml`:

```toml
[plugins]
postgres = "prova-rs/prova-postgres@main"   # owner/repo@ref | local path | { git|path, tag|branch|rev, module }
```

Official plugins: postgres, mysql, redis, kafka, pulsar, rabbitmq, s3. Built-ins: `fs`, `shell`,
`net`, `http`, `grpc` (needs server reflection), `graphql`, `yaml`, `docker`, `sqlite` — plus
`archetect`, a bundled plugin (always present in the standalone binary). No plugin for it? Compose `docker.run{ image, env, ports, wait }` +
`container:run(argv)` + `prova.retry` — or author a plugin via `prova.containerized`.

## Built-ins, one line each

`shell.run(cmd, {cwd, env, timeout, check}) → { code, stdout, stderr } + :ok()`;
`shell.spawn(cmd_string, {cwd, env}) → proc` (`proc.pid`, `:stop()`, `:running()`, `:output()` —
last 64KB of combined output; command is a string, not argv). `fs`: `read write exists glob
tempdir remove_all` (relative paths resolve against the invocation cwd). `net.free_port()`.
`http.get/post/...(url, {headers, json, timeout}) → response` (`.status`, `.body`, `:json()`;
userdata — not table-iterable), `http.client{ base_url }`, `http.wait_for(url, {status, timeout,
every})`. `grpc.client(addr)` (`:call`, `:call_status`), `grpc.wait_for`. `graphql.client{ url }`
(`:query`, `:execute`). `yaml.parse/parse_all`. `sqlite.client(url)`. `docker.run{...} →
container` (`:host_port`, `:run(argv)`, `:exec`, `:logs`, `:stop`), `docker.build{...} → image`,
`docker.network{...} → network`. `archetect.render{...}` /
`archetect.verify(...)`. When unsure of a shape: probe it with `eval` — that is what it is for.

## Boot-then-probe: the quiet idiom

```lua
shell.run("cargo build", { cwd = dir, timeout = "600s", check = true })  -- errors carry BOTH streams
local port = net.free_port()
local proc = ctx:manage(shell.spawn(app_binary, {
  env = { PORT = port, DB_HOST = db.host, DB_PORT = db.port },           -- scalars: no tostring()
}))
http.wait_for("http://127.0.0.1:" .. port .. "/health", { timeout = "60s" })  -- gate, never sleep
-- if readiness fails: proc:output() has the app's combined stdout/stderr (last 64KB)
```

Readiness is always a condition that HOLDS (a query succeeding, an endpoint answering), never a
sleep. Assert effects where they land: probe the API **and** cross-check the database.

## Topologies: one definition, every verb

```lua
local env = prova.topology("orders", function(ctx)
  local db = require("postgres").container(ctx)
  db.client:execute("create table orders (id int, sku text)")
  return { db = db }
end)
```

Tests `t:use(env)` it; `prova up orders` holds the same environment live (prints endpoints, tears
down on Ctrl-C); `prova start/down/ps` manage it detached; `prova watch` re-applies on change.
Your tests and the dev environment are one description — they cannot drift.

Inside a topology factory (and ONLY there) `ctx.network` is an ambient managed network: resources
auto-join it, aliased by recipe name. That gives each resource a second address — `res.network =
{ url, host, port, alias }`, the alias + CONTAINER port that **in-network** consumers use. `res.url`
(127.0.0.1 + mapped port) stays the address **the test runner** uses. Both are live at once.

## The SUT in a container: `build` instead of `image`

The system under test is not a special concept — it is a resource whose image is **built**:

```lua
local app = prova.containerized{
  name = "app",
  build = { context = ".", dockerfile = ".platform/docker/local/Dockerfile" },  -- the REAL prod image
  port = 8080,
  env = function(opts) return { DATABASE_URL = opts.database_url } end,
  url = function(hp) return "http://127.0.0.1:" .. hp end,
}.container(ctx, { database_url = db.network.url })   -- wire via the NETWORK vantage, not db.url
```

The host then needs **nothing but Docker** — `requires = { "docker" }`, no SDK/JVM/uv — and you test
the production artifact. Drive it from the host over `app.url`; cross-check the DB over `db.url`.
Wiring an in-network consumer to a resource's *host* url is the classic mistake: inside a container
`127.0.0.1` is that container. `docker.build{ context, dockerfile?, tag?, buildargs? } → image` is
the primitive underneath (BuildKit + `.dockerignore`); a host-run SUT (`shell.spawn` + host urls)
remains equally valid — pick per fixture.

## Running: selection is your scalpel

```
prova                       # the whole suite (prova.toml, found by walking up)
prova -k MySQL              # only nodes whose path mentions MySQL (repeatable; !PAT excludes)
prova --tags '!build'       # skip a tier by tag (own or inherited from groups)
prova --node "exact › path" # precisely the node a report named
prova --last-failed         # exactly what was red last run — your main iteration verb
prova --list                # discover without running (respects selection)
prova --specs               # only spec-flagged tests — the open backlog (--strict-specs to drive)
prova eval 'return require("postgres").container(ctx).url'   # one-shot probe, auto-teardown
```

`eval` runs in the full environment **with a real `ctx`** — `ctx:manage`/`ctx:defer`/`ctx:tempdir`
all work, and everything the snippet provisions is torn down when it returns (success or error).
Probing a live container's URL, spawning-and-poking a process, dress-rehearsing a fixture: all
safe, all self-cleaning.

Selection pulls dependencies in automatically, keeps flows atomic, and never provisions fixtures
for deselected work. Deselected ≠ skipped: summaries say `N deselected`.

CI: `prova --profile ci` (profiles overlay `[run]`), `--format json` (JSONL events) or `tap`,
`--junit path.xml` (or `[run] junit = "path"` in the manifest). Inside GitHub Actions prova
auto-emits `::error` PR annotations (with file:line) and a step-summary table — no flag needed
(`--gha off` / `PROVA_GHA=off` disables). Console output colors on a TTY only (`--color`,
`NO_COLOR`); `-q` prints failures + tally only. Every failure line and JSON event carries the
test's `file:line`. **The suite you iterate against locally is byte-identical to the one CI
enforces** — that is the point.

## Driving Prova

Two transports, one contract: as a CLI, run `prova <verb>`; as an MCP server (`prova mcp`, stdio),
call tools. Tools mirror the CLI one-to-one and **everything else is identical**:

| MCP tool | CLI equivalent |
|---|---|
| `run { keywords?, keyword_excludes?, tags?, tag_excludes?, nodes?, last_failed?, specs?, strict_specs?, profile?, jobs?, topology? }` | `prova -k … --tags … --node … --last-failed --specs --strict-specs --profile … --jobs …` |
| `list { same selection fields }` | `prova --list` (same flags) |
| `eval { code, topology? }` | `prova eval '<code>'` |
| `learn { topic? }` / `introspect { filter? }` | `prova learn [<topic>]` / `prova.help(...)` in eval |
| `up { name }` / `down { name }` / `status {}` | `prova up <name>` — but held *inside* the server |

Scaffolding stays CLI-only: `prova init`, `prova ide setup`, `prova plugin lint` — shell out for
those even when driving the MCP. Prefer the MCP tools for iteration (warm topologies, structured
JSON); the CLI is the bootstrap surface and what CI runs.

The server resolves the manifest and plugins from its working directory exactly like the CLI,
serves this document as its `instructions`, and returns compact JSON results.

**Warm re-runs — the MCP-only capability.** `up { name }` provisions a named topology once and
holds it inside the server; `run { topology = name }` and `eval { code, topology = name }` then
resolve the held live instance instead of provisioning — millisecond re-runs against a standing
environment while you iterate. In a warm `eval`, the held value is also a global named after the
topology (`return orders.db.url`). Warm calls require a prior `up` (never provision implicitly);
`status` lists what's held; the holder owns teardown — `down` (or server shutdown) reaps, warm
runs never do. A held environment accumulates state (that's the point): `down` then `up` when
isolation matters.

Full reference: https://prova-rs.github.io (source: https://github.com/prova-rs/prova)
