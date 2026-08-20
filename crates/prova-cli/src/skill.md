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
3. Write the proof in a `*.prova.lua` file (`*_test.lua` also accepted) **in a directory matching the manifest's `[run] proofs`
   patterns** (directory names, `["proofs"]` by default — `prova learn project` names this
   package's). Red is correct at this stage.
4. Implement. Re-run with `prova --last-failed` until green. Never weaken a proof to pass it —
   fix the system, or renegotiate the bar with the human. What driving it green surfaces and you
   do not fix now goes to the capture block below before the work moves on — never into this change.
5. Commit suite + implementation together: a proof-carrying change.

**Capture — when the user says "add …", the move is already decided.** Work surfaces obligations
faster than it discharges them; what you find and do not fix NOW is captured in its lane — never
scope-crept into the current change, never a mental note:

- "add it to the **backlog**" → `prova specs capture <id> "<prose>" --file <doc>` (over MCP: the
  `capture` tool) — the verified write into the spec doc whose subject fits: it refuses unscanned
  paths and duplicate ids, stamps the date, and rescans to prove the anchor landed. Captured in
  place, deliberately not yet owed. `prova learn backlog`.
- "add a **claim**" / "spec this in prose" → a `<!-- claim: id -->` anchor in the same docs —
  owed the moment it is written; a proof discharges it via `covers`. `prova learn claims`.
- "add a **promise**" → author the proof now, flagged `promises = "reason"` — the red-by-design
  body IS the record and the spec is executable. Prefer this over prose whenever the contract
  can be stated as a proof today. `prova learn promises`.
- "add a **proof/test**" → a `*.prova.lua` file in a `[run] proofs` directory. `prova learn authoring`.
- "add a **reminder**" → `prova.remind("id", { ... })` watching a condition the future must
  satisfy. `prova learn reminders`.

Never guess placement or syntax from repo archaeology: `prova learn project` names THIS package's
writable spec sources, its proof directories, and the house rules its context carries. Then verify
the capture landed with the lane's own verb — `prova specs --backlog`, `prova owed`,
`prova tests --promises`, `prova reminders`.

**Promises — the executable spec surface.** A contract you can state but are NOT implementing right now
is still worth a proof: author it flagged `{ promises = "reason/ticket" }` (test/flow-level) —
the test states what it will prove someday and does not prove today. Open promises report as
their own outcome (PROMISED) and keep CI green; the moment the body passes it FAILS demanding
graduation — change `promises` to `proves = "<context>"` (a tense change; the why lives on in
the test) or remove the flag — so implementation + graduation land as one proof-carrying change.
`prova tests --promises` enumerates the open surface — an empty list means burndown complete; found
some in a repo? That is scoped work — offer to burn it down with `prova tests burndown`.
`prova learn promises` carries the lifecycle.

**Falsifiers — proving the proof can fail.** A proof that has only ever been green is not
evidence: it may be checking the contract, or checking nothing (an assertion over a value that
cannot vary, a bar a stub already satisfies), and the two are indistinguishable in a report.
Declare the mutation that MUST break it — `{ falsified_by = function(t) … end }` — and
`prova tests falsify` applies it and inverts the verdict: red is the passing result. A body that
survives its own falsifier is reported **vacuous** and fails the run. Reach for one when a proof
asserts an *absence* (no anonymous control, no leaked handle), and especially when the
implementation was written after the proof — a stub that refuses everything satisfies a great
many careless assertions. Costs nothing on the ordinary path; the mutation runs only under the
verb that asks for it.

**Claims — obligations that arrive from outside prova.** Specs also come from design docs and
tickets, and an agent can say it implemented one without having done so. A `<!-- claim: id -->`
anchor in prose is an obligation; `covers = "docs/design.md#id"` on a proof discharges it; and
`prova owed` reconciles every origin into ONE list — open promises, unproven claims, and `covers`
pointing at prose that is not there. Pin a claim's text (`prova owed --pin`) where the exact
wording is the contract, and an edit reports STALE instead of passing silently. Opt in with
a `[specs]` source (`[[specs.source]] type = "directory"`); absent, the subsystem is inert. If you find things owed in a repo,
surface them — that is scoped work someone has not finished.

**Evidence — where does this project stand?** The doc *claims* it; a proof *promises* it; the
implementation *proves* it; the run *attests* it. `prova evidence` is the whole account: CLAIMED /
BOUND / PROMISED / ATTESTED with counts, then what is owed. Start here when orienting in a repo
that declares `[specs]`; `prova owed` is the actionable narrowing. In CI, **bare `prova attest`
gates**: it reconciles every anchored claim against the recorded run and exits non-zero unless
each is attested. `prova learn evidence` carries the family.

**The run record — never report `0 failed` as "covered".** It is equally true of a suite that
proved everything and one in which every proof **skipped** for want of a docker daemon, a broker
or a display. Each run writes `.prova/var/last-run.json` naming — individually, not summed — the
skipped (with the gate's reason) and the deselected. **Before claiming an obligation is done, run
`prova attest docs/design.md#id`**: it exits non-zero unless a proof covering that address
actually executed and passed, so skipped, deselected, absent, red and still-promised all read as what
they are — no evidence. `--record <path>` also emits it for CI to keep. `prova learn record`.

Prova complements the language's own test harness; it does not replace it. Prove the CONTRACT
with prova (behavior a real caller observes at the boundary); prove the INTERNALS with native
unit/integration tests (one function's logic, seams the boundary can't reach). A change often
needs both — the right tool for each job; `prova learn pdd` carries the decision table.

## Learning on the fly: never guess, ask the binary

Everything below is the crash course; depth is one call away, computed for THIS package:

| You need | Move |
|---|---|
| The topic catalog (patterns, doubles, topologies, package authoring…) | `prova learn` · MCP `learn {}` |
| One topic (aliases work: `mocks` → `doubles`) | `prova learn <topic>` · `learn { topic }` |
| An API's shape: what to call, what comes back | `prova.help("<filter>")` in any test/eval · MCP `introspect { filter }` |
| Which archetypes `init` can scaffold | `prova init --list` (or `prova learn init`) |
| A live value's shape | probe it with `eval` |
| The open promises (proofs ahead of implementation) | `prova tests --promises` · `prova learn promises` |
| Whether the proofs can actually fail (vacuous-proof hunt) | `prova tests falsify` · `prova learn falsify` |
| Where the project stands — the whole account | `prova evidence` · `prova learn evidence` |
| Everything this package owes, from every origin | `prova owed` · `prova learn claims` |
| Whether a claim's proof actually RAN (not just "0 failed") | `prova attest <doc.md#id>` · `prova learn record` |
| A package for a technology you need to prove | `prova packages <term>` (search registries) → `prova packages add <name>` |

## Test files, in one screen

Files match `*.prova.lua` — preferred, since a file may declare tests, fixtures, topologies, and reminders — plus the accepted `*_test.lua` / `*.test.lua`. Everything is a global — no imports except packages.

```lua
local postgres = require("postgres")          -- a dependency, declared in prova.toml [dependencies]

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
  `locks = { prova.port(N), prova.writes("db"), prova.reads("cache") }` (say what the test does
  to the token: `writes` = exclusive, `reads` = concurrent; held across every prova instance at
  this home, so house rules like "one cargo at a time" survive `-j` and concurrent runs —
  `prova learn locks`), `serial = true` (run-scoped), `falsified_by = fn` (the mutation that must break it — `prova tests falsify`),
  `promises = "reason"` (a
  proof authored ahead of its implementation — `prova learn promises`). `--jobs` is throughput
  only — it can never change what a run means.
- Context: `ctx:use(handle)`, `ctx:manage(resource)` (auto stop/close at scope end),
  `ctx:defer(fn)`, `ctx:tempdir(name?)` (this scope's scratch dir — the SAME one for the same
  name, every call; name it when you need several, and the name lands in the path so a failed run
  is readable on disk), `t:expect(v, label?)`, `t:expect_all(fn)` (soft), `t:skip(why)`.
- Matchers, by what the SUBJECT is (a flat list hides which ask the filesystem) — negate with
  `:never()`:
  - any value: `equals is is_nil contains matches has_length is_one_of exists`
  - booleans, and the pair matters: `is_true`/`is_false` are STRICT (the boolean itself — `nil`
    fails), `is_truthy`/`is_falsy` are Lua truthiness (`0` and `""` are TRUTHY). Reaching for the
    loose one to assert a strict fact is how an assertion passes on `nil`.
  - numbers: `gt gte lt lte` · paths: `is_file is_dir is_fully_rendered` · trees: `matches_snapshot`
  - `exists`/`is_empty` are polymorphic — present/empty *for whatever the subject is* — but a
    **string is resolved as a path** (asserting a file is there is the load-bearing use). For a
    string's presence use `never():is_nil()`.
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

Every service resource — package or hand-rolled — is the same shape: **`X.client(...)` attaches to
something running; `X.container(ctx, opts?)` provisions ephemerally and returns
`{ client, url, container, host, port }`**. `url` is what you inject into the app under test;
`host`/`port` split it for discrete env vars. Declare dependencies in `prova.toml`:

```toml
[dependencies]
postgres = "prova-rs/prova-postgres@main"   # owner/repo@ref | local path | { git|path, tag|branch|rev, module }
```

Official packages: postgres, mysql, redis, kafka, pulsar, rabbitmq, s3. Built-ins: `fs`, `shell`,
`net`, `http`, `grpc` (needs server reflection), `graphql`, `yaml`, `docker`, `sqlite` — plus
`archetect`, bundled (always present in the standalone binary). No package for it? Compose `docker.run{ image, env, ports, wait }` +
`container:run(argv)` + `prova.retry` — or author a package via `prova.containerized`.

## Built-ins, one line each

`shell.run(cmd, {cwd, env, timeout, idle_timeout, first_byte, check, merge_stderr, stdin}) →
{ code, stdout, stderr } + :ok()`; `shell.spawn(cmd, {cwd, env}) → proc` (`proc.pid`, `:stop()`,
`:running()`, `:output()` — last 64KB of combined output). **Both take a shell string OR an argv
table** (`{"kubectl","get","pods"}` — no shell, no quoting); there is no `args` option, because the
arguments are part of the command. `fs`: `read write exists glob tempdir remove_all` (relative
paths resolve against the invocation cwd). `net.free_port()`.
`http.get/post/...(url, {headers, json|form|body, content_type, timeout, redirects}) → response`
(`.status`, `.body` — raw bytes, exact for binary — `:json()`, `:save(path)`; userdata, not
table-iterable), `http.client{ base_url, headers, timeout }`, `http.wait_for(url, {status, headers, timeout,
every})`. `grpc.client(addr)` (`:call`, `:call_status`), `grpc.wait_for`. `graphql.client{ url }`
(`:query`, `:execute`). `yaml.decode/decode_all`. `sqlite.client(url)`. `docker.run{...} →
container` (`:host_port`, `:run(argv)`, `:exec`, `:logs`, `:stop`), `docker.build{...} → image`,
`docker.network{...} → network`. `docker.run{ files = { ["/abs/path"] = { text|file|dir = … } } }`
carries configuration INTO a container between create and start — no image build, and not a bind
(the bytes travel the daemon API, so a remote daemon works). `archetect.render{...}` /
`archetect.verify(...)`. When unsure of a shape: probe it with `eval` — that is what it is for.

**A ratchet that refuses you is pointing at a seam.** The quality gates (file size, function
length, clone count, unwrap census, the coverage floor) do not suggest — they refuse, and the way
through is to find the boundary the code already wanted. Rebaselining exists and is almost never
the answer. Paying a floor down is a DISCOVERY exercise, not a number to clear: writing real tests
for the least-covered code is how latent defects surface, so do that rather than covering lines
cheaply — a green gate that means nothing is worse than a red one that means something.
`prova learn pdd` has the worked evidence.

**Option tables are closed.** Every one of these refuses a key it cannot honor, naming the key, the
nearest accepted spelling, and the accepted set — it is never silently dropped, because a dropped
option reads as *configured* (`tiemout = "10m"` means unbounded). Two consequences worth knowing:
a refusal is a **fast, exact answer about the API**, so guessing a key and reading the error beats
searching for the right one; and a refusal naming an option you are sure exists means the binary
under test is OLDER than the proof — which is the loud version of a proof that used to pass while
proving nothing.

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
prova --node "exact › path" # precisely the node a report named (implies its switch, if any)
prova -s heavy              # throw an opt-in switch: include the `heavy` class this run
prova --last-failed         # exactly what was red last run — your main iteration verb
prova tests                 # the tests lane, state-tagged PROMISE/PROOF (respects selection)
prova tests --promises      # just the open spec surface · `prova tests burndown` drives it red-loud
prova --promises            # the composable selector underneath (only promised tests)
prova eval 'return require("postgres").container(ctx).url'   # one-shot probe, auto-teardown
```

**Wiring CI: guarantee, don't degrade.** A test's `requires = { "docker" }` SKIPS when the
capability is absent — right on a laptop, wrong on a merge gate, where a box that lost its Docker
daemon then reports a green "0 failed" having proven nothing. A typo'd capability name skips just
as quietly. Name what the environment promises in the manifest and an unmet one fails the run up
front, before any test executes:

```toml
[run]
must_run = ["node"]                    # true everywhere; a run without it proves nothing

[profiles.ci]
must_run = ["docker", "dotnet >= 9"]   # CI promises these — unmet is a broken box, not a skip
```

Guarantees are **unioned** with `[run]`'s and can never be subtracted, so `prova --profile ci`
demands both sets. Same expression grammar as `requires`, same probes. Related, same hazard: a
selection matching nothing exits non-zero rather than reporting `0 passed`.

**Declare the prova a suite needs** with `[requires] prova = ">= 0.13"`, and an older binary says
so up front instead of failing mid-run on a missing feature. Write `>=` — the value is a semver
range, so a bare `0.13` means `^0.13`, which on 0.x refuses 0.14 and later.

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
| `run { keywords?, keyword_excludes?, tags?, tag_excludes?, nodes?, switches?, last_failed?, promises?, due?, profile?, jobs?, topology? }` | `prova -k … --tags … --node … -s … --last-failed --promises --due --profile … --jobs …` |
| `list { same selection fields }` | `prova --list` (same flags) |
| `eval { code, topology? }` | `prova eval '<code>'` |
| `evidence { package? }` / `owed { package? }` | `prova evidence` / `prova owed` — the account and its debts |
| `attest { address, package? }` | `prova attest <addr|id>` — bare `prova attest` (the CI gate) is CLI-only |
| `learn { topic? }` / `introspect { filter? }` | `prova learn [<topic>]` / `prova.help(...)` in eval |
| `up { name }` / `down { name }` | `prova up <name>` — but held *inside* the server |
| `status { package? }` | `prova ps` — what is held: the server's own warm holds, plus the package's detached ones |

Scaffolding stays CLI-only: `prova init`, `prova ide setup`, `prova package lint` — shell out for
those even when driving the MCP. Prefer the MCP tools for iteration (warm topologies, structured
JSON); the CLI is the bootstrap surface and what CI runs.

The server resolves the manifest and dependencies from its working directory exactly like the CLI,
serves this document as its `instructions`, and returns compact JSON results.

**Warm re-runs — the MCP-only capability.** `up { name }` provisions a named topology once and
holds it inside the server; `run { topology = name }` and `eval { code, topology = name }` then
resolve the held live instance instead of provisioning — millisecond re-runs against a standing
environment while you iterate. In a warm `eval`, the held value is also a global named after the
topology (`return orders.db.url`). Warm calls require a prior `up` (never provision implicitly);
the holder owns teardown — `down` (or server shutdown) reaps, warm runs never do. A held
environment accumulates state (that's the point): `down` then `up` when isolation matters.

**Before you provision, ask `status` — and aim it.** It reports both holders: this server's warm
holds, and the DETACHED ones a terminal `prova up` / `prova start` holds, which live as records
under the package they belong to. So `status { package = "<dir>" }` is how you see the second kind
— and if you skip the `package`, a server whose config is per-user (started in a home directory,
not a repo) can only answer for its own warm holds. `packages` in the result names what it read;
an EMPTY `packages` is the one case where `held: []` does not mean "nothing is up". Getting this
wrong costs a cold stand-up of an environment that was already up, plus a port collision if it
binds fixed ports. `up` now refuses that stand-up rather than performing it — for a detached holder
it names the pid and the two exits (`prova down <name>` in that package, or CLI `prova --topology
<name>` to run against the live instance), since a hold it did not create is not its to reap.

Full reference: https://prova-rs.github.io (source: https://github.com/prova-rs/prova)
