# Prova — Runtime Architecture

> Companion to [`api.md`](api.md) (authoring surface) and [`foundations.md`](foundations.md)
> (the thesis). This doc is the *engine*: async model, the definition→plan→execute pipeline,
> output as a plugin surface, and the frontend protocol that lets a GUI/IDE drive the same core
> the CLI does. Status: **early implementation** (`crates/prova-core`). Not all of this is built
> yet; what is built is noted, and everything here is a decision the built parts already respect.
> For live, package-computed facts prefer the autodidact rails: `prova learn` (topics),
> `prova.help()` / MCP `introspect` (API shapes).

## Principles (why the engine is shaped this way)

1. **Definition ≠ execution.** Collection produces an inert node tree; a separate pass *runs* it.
   A different driver (a load/stress executor) can run the same bodies under a different regime.
   *(Built: `build_lua` collects; `build_plan` flattens; `run_plan` executes.)*
2. **Re-runnable, context-injected bodies.** A body is stored as an `mlua::Function` and invoked
   with a freshly built context each time. This one property is the precondition for retries,
   property-based shrinking, and load-loops. *(Built: `run_one` builds a fresh `t` per invocation.)*
3. **Async from the ground up.** Bodies are driven with `call_async`, so a body can `await` I/O
   without blocking a thread. *(Built.)*
4. **Output is a plugin surface, never a hardcoded printer.** Execution emits a structured event
   stream; sinks consume it. *(Built: `Event` + `Reporter` + `MultiReporter` + `JsonReporter`.)*
5. **The CLI is just one frontend.** Discovery + a streaming event protocol let a GUI/IDE drive
   the identical core. *(Built: `discover_path`, JSONL event protocol.)*

## Async execution model

- **Per-run runtime.** Each run spins a `current_thread` Tokio runtime and `block_on`s the plan.
  No global runtime; the engine is a library first.
- **Cooperative single-state concurrency (now).** One Lua state drives *many* bodies concurrently
  via `buffer_unordered(concurrency)`. Because Lua is cooperative, exactly one body executes at any
  instant; when a body `await`s (I/O, sleep), control returns to the runtime and another proceeds.
  This is the k6/JS event-loop model and is ideal for the **I/O-bound concurrency** that
  acceptance flows and load tests need. *(Built; proven: two 40 ms sleeps complete in ~45 ms wall,
  not ~80 ms.)*
- **Per-worker states for multi-core, dispatched by *suite* (built).** True CPU parallelism needs
  more than one thread, and an `mlua::Lua` is best kept to one thread — and its `Function` bodies are
  `!Send`, so a body collected on one state cannot run on another. The realized design: **N workers =
  N OS threads, each with its own Lua state**, with the **suite** as the dispatched unit — a worker
  loads a suite (its `suite.lua` setup + all member files) into one state and runs it end to end;
  within a worker, cooperative async as above. **An ungrouped file is a singleton suite**, so the
  default is exactly per-file parallelism. Grouping files into a suite (a directory's `suite.lua`)
  makes them share one state — which is what makes `Scope.Suite` a live cached value across the files,
  no serialization. Suites run in parallel; `--jobs` sets the worker count (concurrent *suites*) and
  is **throughput-only, never semantic**. *(Built in `suite.rs`; see [`suites.md`](suites.md).)*
- **`!Send` is fine.** Bodies, contexts, and fixtures are `Rc`/`RefCell` (single-thread). We use
  `FuturesUnordered`/`buffer_unordered` (poll-in-place, no `spawn`), so nothing needs `Send`.
  Cross-worker sharing (a `suite` fixture) will be an explicit, serialized handoff, not implicit.

## definition → plan → execute

```
collect (run the .lua file)        →  Node arena (groups/flows/tests + fixtures)
  build_plan (flatten + expand deps)→  Plan { leaves: [Leaf{unit, deps}] } — a leaf DAG
    run_plan (scheduler)            →  drives bodies deps-first, emits Events, tallies Summary
```

The **plan** is where strategy is resolved: a group flattens to its leaves (independent,
parallelizable), a flow becomes one leaf whose steps are an ordered sub-run on one worker,
`depends_on` becomes leaf edges that gate/order, and `resources` become the readers-writer
constraints the scheduler co-schedules against. Keeping the plan a distinct artifact is what lets a
**load executor** be a drop-in alternative to the acceptance executor over the same leaves.

## Timeouts (the three mechanisms)

You cannot safely kill a running Lua VM mid-instruction, so timeouts are layered:

1. **Async I/O deadline (built).** A body's future is wrapped in `tokio::time::timeout(budget, ..)`;
   when it elapses, the future is cancelled and the unit fails with `timed out after …`. This
   covers the common case — a body wedged on a slow/hung HTTP call, DB, or subprocess.
2. **CPU-bound interrupt (planned).** A `while true do end` never awaits, so it never hits the
   deadline. An `mlua` interrupt hook, armed with the unit's deadline, will abort at an instruction
   boundary.
3. **Process containment (planned).** For truly wedged native calls, the per-worker model can
   escalate to process-per-worker with a hard kill.

Budgets nest: effective deadline = min(step, flow, suite, global). Teardown still runs after a
timeout (best-effort, itself bounded).

## Output as a plugin surface

- **`Event`** — the structured stream: `RunStarted`, `NodeStarted{path}`,
  `NodeFinished{path, outcome, duration, assertions, message, file, line}`, `RunFinished{summary}`.
  The executor only *emits*; it never prints. `file`/`line` are the declaration's source location,
  captured from the Lua stack at registration (`None` for file-less runs like `eval`).
- **`Reporter`** — one method, `event(&Event)`. Core implementations (unstyled by design):
  `ConsoleReporter` (plain fallback), `JsonReporter` (JSONL wire protocol), `JUnitReporter` (CI
  file sink: locations, timestamp, properties), `TapReporter` (TAP 13), `NullReporter` (tests /
  load driver). The CLI layers presentation in `prova-cli/src/report.rs`: `HumanReporter` (a
  streaming *tree* — file → group/flow → leaf, rendered by transition so sequential runs print
  each header once and parallel interleaving honestly reprints; color via anstream with auto
  TTY/`NO_COLOR` detection, skip reasons, failures recap, `--quiet`) and `GitHubReporter`
  (auto-on under `GITHUB_ACTIONS`: `::error` annotations + step summary).
- **`MultiReporter`** — fan-out, so console + a JUnit writer + the GitHub sink run simultaneously.
- Planned sinks: a load-metrics aggregator (consumes the same stream, emits latency
  histograms/percentiles instead of pass/fail lines).

## Snapshots (folded from docs/plans/snapshots.md — landed A+B+C)

`t:expect(subject):matches_snapshot([key], { level })` compares against a reviewable `.snap`
colocated with the test (`<dir>/snapshots/<stem>__<key>.snap`; key = explicit name, else a slug
of the node path + a per-test counter). `-u/--update-snapshots` (re)writes; a mismatch fails
with a line diff; a missing snapshot fails and writes a `.snap.new` for review (insta parity).
The **level** is the strictness dial and the anti-rot default lives in the API: `layout`
(sorted relative paths — files added/removed/moved, low rot) is the default for tree subjects;
`content` (paths + bytes) is the opt-in golden-file mode. The matcher stays generic through a
snapshot *protocol*: a snapshottable handle serializes itself at a level; strings are their
bytes. Discipline: a run-wide registry of referenced `.snap` files + `--unreferenced
ignore|warn|delete` flags orphans — sound only on FULL runs (a filtered run would make unrun
tests' snapshots look orphaned, so the check skips with a note).

## Frontend protocol (the companion GUI/IDE, and the CLI, over one core)

The engine is designed so a **GUI app is not a fork** — it's another consumer of the same two
capabilities the CLI already uses:

- **Discovery** — `discover_path` collects the tree *without executing*, so a frontend can load a
  file and render the test model (for a runnable tree, code-lens "run" affordances, etc.). Today it
  returns test paths; it will return a richer serializable tree (ids, kinds, params, ranges).
- **Streaming events** — the JSONL `Event` protocol is line-delimited and ordered, so a frontend
  subscribes and updates a live view as results arrive (à la the VS Code Test Adapter Protocol /
  DAP). Selective runs (run one test/subtree) map to filtering the plan by node id.

So the split is: **`prova-core`** (engine) → **`prova` CLI** (one frontend) and a future
**Prova GUI** (another frontend) — both speaking discovery + events. LuaCATS annotations already
give the authoring/completion half of the IDE story; this gives the run/report half.

## Plugin surfaces (how "comprehensive without capability ceilings" stays true)

The core stays small; capability grows at the edges:

| Surface | Extends by | Examples |
|---|---|---|
| **Modules** | async Lua modules registered into the runtime (via `RunConfig::with_module`) | `fs`, `net`, `shell`, `http`, `grpc`, `graphql`, `docker`, `sqlite`, `yaml`, `archetect` (resource clients are external plugins) |
| **Matchers** | new terminal checks on the matcher | domain assertions, snapshots |
| **Reporters** | new `Reporter` sinks | JUnit, TAP, GUI socket, load metrics |
| **Selectors** | plan filters | tag expressions, `--changed`, `--last-failed`, sharding |
| **Executors** | alternate drivers over the plan | acceptance (once), **load/stress** (many, sustained) |

**`docker`** — ephemeral dependencies, testcontainers-style — is **built** (`docker.run` → a
`Container`, the same managed-lifecycle pattern `shell.spawn`'s `Process` established), paired with
`requires` gating so a suite skips where Docker is absent. **`grpc`** is now built too — a native,
dynamic, reflection-driven client (no `grpcurl`, no `.proto` files), the same async-module shape as
`http` (call a method, assert on the response/status). The network-interface trio is complete:
**`http`** (REST, + `http.client`), **`grpc`** (native/dynamic), **`graphql`** (`graphql.client`
with `query`/`execute`).

The end-to-end arc the modules serve is now real through the network-drive step: **render the
project → assert the layout → boot the app (`shell.spawn`) → spin up its dependencies (`docker`) →
drive its network interfaces (`http`/`grpc`/`graphql`) → tear it all down** — one framework,
no capability ceilings.

These seams keep options open — e.g. the definition≠execution split means a load/stress driver
*could* run a `flow` as a scenario under a concurrency/duration profile over the same plan and event
stream, with no new authoring surface. We note this as evidence the layering is clean, **not** as a
feature on the roadmap: load/performance testing is an explicit **non-goal** (see `foundations.md` —
it stays with k6/Gatling; prova asserts behavioral correctness, it does not model load). The door is
left open by good architecture; we are not walking through it.

## Current status (implemented)

- Async collect→plan→execute for `prova.test` / `prova.group` / `prova.flow`; injected `prova`
  global. All three (and the builder variants) accept an optional `opts` table and return unit
  handles for `depends_on`.
- **Table-driven tests**: `prova.test_each(name_tmpl, cases, fn)` (and `GroupBuilder:test_each`)
  generate one test per case, filling `{placeholder}`s in the name from the case; the case reaches
  the body as its second argument *and* as `t.case` (an optional `case` threaded through
  `Node → PlanItem → Ctx`, so `fn(t, case)` and plain `fn(t)` both work). *(`testdata/test_each.lua`.)*
- **Labeling groups**: `prova.describe(label, fn)` nests bare `prova.test`/etc. inside `fn` under
  `label` via a `parent_stack` in the collector (dynamic scoping; popped even on error);
  `GroupBuilder:describe` is the builder form. Labeling only — no new fixture scope.
  *(`testdata/describe.lua`. Used by `examples/rust_cli_test.lua`.)*
- **Fixtures + scopes + teardown**: `prova.fixture(name, scope, factory)` → typed handle;
  `ctx:use(handle|name)` builds-or-caches; scope is a **typed `Scope` value** (`Scope.Test` default /
  `Scope.Flow` / `Scope.File` / `Scope.Suite` — no magic strings) with per-scope caches; `ctx:defer`
  (LIFO); `ctx:tempdir` (auto-removed); scope-mismatch rejection; inner→outer
  teardown. **`ctx:use` is async** — a factory can `await` (e.g. `shell.run`, a readiness poll);
  recursion reenters through Lua, so no boxing. **Teardown is async too** — a `ctx:defer` callback
  can `await` (e.g. `proc:stop()`), reaped while the runtime is still alive. **`ctx:manage(resource)`**
  is the ergonomic form: it ties a resource's lifecycle to the scope (auto `stop()`/`close()` on
  teardown) and returns it, so `local pg = ctx:manage(docker.run{...})` provisions + registers cleanup
  in one line — no `ctx:defer(function() x:stop() end)` closure. *(`lifecycle_poc_test`;
  `async_fixture`; `service_lifecycle_test` boots a process and stops it on teardown, leak-free.)*
- **Readiness without ceremony**: `prova.retry(fn, { timeout, every, message })` calls `fn` until it
  returns truthy (a raise = "not yet") or the deadline elapses, returning the value — replacing the
  hand-rolled `for _=1,N do pcall(...) sleep end` loop (`local conn = prova.retry(function() return
  sqlite.client(url) end)`). *(`testdata/ergonomics.lua`.)*
- **Resource clients are external plugins** *(as of the 2026-07-15 extraction)*. Databases, caches,
  brokers, object stores, and streams — every *containerized* resource — live as **external docker-exec
  plugins** under `prova-rs/prova-<name>` (redis, postgres, mysql, s3, kafka, pulsar, rabbitmq, …),
  authored through `prova.containerized` + `container:run` and fetched via `prova.toml`. Core compiles
  **none** of them in (no sqlx-for-servers / rdkafka / pulsar / rust-s3), so the binary is lean and no
  technology is privileged over another. Each drives the CLI already in the image (`redis-cli`, `psql`,
  the `mysql` CLI, `mc`, the kafka console tools, `pulsar-client`) and self-tests in its own repo.
  Generated from `prova-rs/prova-plugin-archetype`. See [ecosystem.md](ecosystem.md).
- **`sqlite` stays embedded** — the one bundled resource client, because it is *not* a container:
  `sqlite.client(url)` (`sqlite::memory:` or a file) over sqlx, no docker needed — the fast in-process
  database for tests that don't want a container.
- **`docker.run{ command }`** — override the image CMD (a string, whitespace-split, or a list), needed
  by images like Pulsar's (`bin/pulsar standalone`). **`docker.run{ ports = { { container, host } } }`**
  — a *fixed* host port (else random), needed by Kafka (its advertised listener). Both surface through
  `prova.containerized` (`spec.command`, a `{ container, host }` ports entry), which every resource
  plugin is built on.
- **Deferred: attach-to-secured-external / TLS+auth.** Everything connects **plaintext** in v1 — right
  for the local/CI ephemeral-container mission, but a secured *remote* endpoint (a dev k8s cluster /
  cloud broker with TLS + tokens) needs auth. In the plugin model this is a per-plugin concern (a
  client-container or a native option) plus the network-drive primitives growing an `https`/TLS
  feature — additive, for when the "point at a dev cluster" environment-testing variation lands.
- **Capability modules** (`modules.rs`), injected as their own globals: **`shell.run(cmd, {cwd,
  env, timeout, check})`** (async via `tokio::process`; returns `{code, stdout, stderr, duration}` +
  `:ok()`) and **`shell.spawn(cmd, {cwd, env})`** → a managed `Process` (`.pid`, `:running()`,
  async `:stop()`/`:wait()`; `kill_on_drop` backstop) — the boot-the-app primitive; **`fs`**
  (`exists`/`read`/`write`/`remove_all`/`tempdir`/`glob`); **`net`** (`free_port()` for a locally-
  spawned app's dynamic port); and **`http`** (`get`/`post`/`put`/`patch`/`delete`/`head`/`options`/
  `wait_for`; async via reqwest; response `.status`/`.body`/`.headers` + `:json()`; `wait_for` is the
  boot-then-probe poll; **`http.client{ base_url, headers }`** is a reusable REST client that prefixes
  the base URL and merges default headers — the ergonomic path for a suite hitting one service).
  Filesystem matchers
  `:exists()`/`:is_file()`/`:is_dir()` take a path-string **or handle-table** subject. This is the
  slice that lets prova test a real rendered workspace and a running service.
  *(`proofs/shell/shell_fs_test.lua`; `proofs/http/probe_test.lua` boots a server + probes it.)*
  `http` is feature-gated (default on) and HTTP-only in v1 — an `https`/rustls feature layers on
  later; the rest of the stack needs no TLS. Also **`docker`** (`docker.run{image, ports, env,
  wait}` → a `Container`: `.id`, `:host_port(p)`/`:endpoint(p)`, async `:logs()`/`:exec(cmd)`/
  `:stop()`) — testcontainers-style ephemeral deps via the typed **bollard** daemon client (not CLI
  parsing): pull, create + start with random host-port bindings, inspect for the mapped ports,
  readiness wait (port TCP-connect or log-substring), `remove_container(force)` on `:stop()` with a
  `Drop` backstop so a container never leaks. *(`proofs/docker/dependency_test.lua`; verified
  against a real daemon — whoami HTTP, redis exec/logs, real Postgres.)*
- **`sqlite` module** — an embedded SQL query API over **sqlx's `Any` driver**: `sqlite.client(url)`
  (`sqlite::memory:` or `sqlite://…?mode=rwc`) returns a `Connection` with async `:execute` (rows
  affected), `:query` (list of column-name→value tables, NULL→nil, typed by SQL kind with a probe
  fallback for computed columns like `count(*)`), `:query_value` (scalar), `:close`. `?`-placeholder
  params bind Lua int/float/bool/string/nil. Needs no docker — the fast in-process database. Feature
  `sqlite` (default on). *(`tests/sqlite.rs`.)* (Server databases — postgres/mysql — are external
  docker-exec plugins; only sqlite is embedded.)
- **`yaml` module** — `yaml.parse(text)` (single document) and `yaml.parse_all(text)` (multi-document
  `---` stream, as in k8s manifests) → Lua values, the counterpart to `http`'s `:json()`. General
  black-box machinery for a cloud-oriented, polyglot world (k8s/CI/compose are all YAML). serde_yaml_ng,
  feature-gated `yaml` (default on). *(`testdata/yaml.lua`.)*
- **`archetect.verify{...}`** *(prova-archetect)* — the declarative archetype check, prova's answer to
  the pytest harness's `manifest.yaml`, matched field-for-field but as real Lua. One call renders once
  (headless) and registers the standard tests: `expected_files`/`absent_files` layout, `is_fully_rendered`,
  `yaml_globs` (each glob matches ≥1 file and each match parses), and a `requires`-gated `build_steps`;
  returns the shared render fixture so callers can add their own tests (the superset pattern). Lua sugar
  over prova primitives + `fs`/`shell`/`yaml`, installed alongside `archetect.render`.
  *(`examples/archetype_verify_test.lua`; verified against the real `rust-grpc-service-archetype@dev`.)*
- **`grpc` module** — a **native, dynamic** gRPC client (no `grpcurl` binary, no `.proto` files, no
  codegen): `grpc.connect(addr)` performs **gRPC Server Reflection** once to learn the server's
  schema, then `client:call("pkg.Service/Method", req_table)` builds the request message from the Lua
  table against the fetched descriptors, invokes it, and decodes the reply to a table;
  `client:call_status(...)` returns `{ok, code, message, response}` for status-code assertions;
  `grpc.wait_for(addr)` is the boot-then-probe poll. Built on `tonic` + `prost-reflect`'s
  `DynamicMessage` with a generic tonic codec; reflection negotiates v1, falling back to the older
  v1alpha many servers still speak. Plaintext-only in v1 (matching `http`'s no-TLS stance);
  feature-gated `grpc` (default on). Chosen over shelling to `grpcurl` to preserve prova's
  single-self-contained-binary promise. *(`proofs/grpc/grpc_test.lua` + `tests/grpc.rs` run the three
  round-trips — unary, field echo, and a `NotFound` status — against a real reflection-enabled server
  (`moul/grpcbin`) in an ephemeral container, gated by `requires` so it skips without a daemon.)*
- **Plugin system** — `require("name")` resolves Lua plugins through a custom `package.searchers`
  entry (`plugins.rs`), in order: **bundled** first-party modules embedded in the binary (`prova.*`),
  **manifest-declared** plugins (`prova.toml` `[plugins]`, authoritative + pinned), then disk — every
  **declared** plugin root from the manifest's `[run] plugin_root` (root-relative; `<a/b>.lua` or
  `<a/b>/init.lua`). **Everything is declared**: no default root, no `PROVA_PLUGIN_PATH`, no
  cwd-relative fallback, no machine-global dir — discovery finds `prova.toml`, and from there the file
  names every place a plugin may come from, so a clean clone resolves what the author's machine does
  and one file answers "where could this require have come from?". A plugin may also declare **private dependencies** in its own `prova.toml` (`[plugins]`)
  (`[plugins]`), which resolve for that plugin's code alone — the "bundled + isolated" model that
  lets a library depend on something without exposing it to consumers. A plugin is authored exactly
  like a first-party recipe: one namespace table following the grammar, composing primitives,
  `return`ed. **XDG `SystemLayout`**
  (`layout.rs`: `config_dir`/`cache_dir`/`data_dir`, XDG on macOS too like archetect;
  `XdgSystemLayout` + `RootedSystemLayout` for tests). **`[plugins]`** maps a name to a local path or
  a git source (`{ git, tag/branch/rev, module }`); git sources are **fetched (shelling to `git`) into
  `cache_dir/plugins`, pinned by ref and cached** (CLI `plugins.rs::resolve_plugins`). Ships one
  bundled loadable namespace, **`prova.workspace`** (`workspace.create(ctx)` → a scratch dir tied to
  the scope via `ctx:manage`, composing `fs`), proving the loadable path first-party recipes will
  migrate onto. *(`proofs/shared/shared_plugin_test.lua`; `tests/plugins.rs` = bundled + disk + clean
  miss; `crates/prova-cli/tests/plugin_git.rs` fetches a git plugin end-to-end through the real
  binary.)* Design: **[plugin-system.md](plugin-system.md)**. Next: migrate a real recipe (e.g.
  `redis`) onto the loadable path; `prova plugin add`; read `~/.config/prova`.
- **`requires` capability gating**: `opts.requires = { "docker", ... }` skips (does not fail) a unit
  when a capability is unavailable, with a reason; the skip cascades to dependents. Detection:
  `docker` → `docker info` succeeds, `github` → `GITHUB_TOKEN` set, else a tool of that name on
  `PATH`. Resolved once per capability at plan time into a leaf `precondition_skip`; the scheduler's
  skip-fixpoint handles it (independent of deps). Inherited from groups like `depends_on`. This is
  what lets a docker suite degrade gracefully where Docker is absent. *(`tests/requires.rs`,
  `tests/docker.rs` — the docker test runs a real container where docker is present, skips where
  not; verified: 2 skipped, 0 failed with no daemon.)*
- **Plugin-module hook**: `RunConfig::with_module(Fn(&Lua) -> Result)` registers extra globals into
  every Lua state the run creates (built-ins `shell`/`fs` are always installed). This keeps
  `prova-core` domain-agnostic while letting the host inject capabilities — the plugin boundary the
  design calls for.
- **`archetect` plugin** (`prova-archetect` crate, kept out of the core): `archetect.render{source,
  answers, switches, defaults, destination}` renders an archetype **in-process** via archetect-core
  — the justifying use case. Headless (never prompts): a `CapturingIoHandle` (a `ScriptIoHandle`)
  writes files, Acks in lockstep, and records the ordered write list; render runs on a dedicated OS
  thread (isolated from prova's Tokio runtime). Returns a **tree handle** table (`out.path`,
  `out:file(rel)`, `out:dir(rel)`, `out:read()`, `out.writes`) that flows into the fs matchers. The
  standalone `prova` binary ships it. *(`examples/archetect_render_test.lua` + a real greeting
  archetype fixture; `tests/render.rs` proves defaults + answer-override in-process and end-to-end.)*
- **Flows**: `prova.flow(name, body)` / `g:flow(...)` register a `Flow` node; `f:step(name, fn)`
  declares ordered steps. A flow is **one scheduling unit** (`PlanUnit::Flow`): steps run serially
  in declared order, sharing closure upvalues (the flow context bag) and a `flow`-scope instance;
  once a step fails the rest **cascade-skip** (skip, not fail; a self-`skip` does not cascade); the
  flow scope tears down after the last step. Flows parallelize with sibling units.
  *(`proofs/flows/lifecycle_test.lua` runs green: shared upvalue, shared flow-fixture, cascade proven.)*
- **Dependency DAG** (`depends_on`): `prova.test`/`flow`/`group` return `UnitHandle`s. `build_plan`
  flattens the tree into leaves (tests + flows; a group is not a leaf) and expands each unit's
  `depends_on` — folding in **inherited** group-level deps — into concrete leaf edges (a dep on a
  group fans out to that group's leaves). Cycles are a collection-time error (defensive; Lua's
  backward handle refs make them practically unreachable). The scheduler runs a leaf once all its
  dependency leaves have **passed**; any failed/skipped dep **cascade-skips** it (transitively,
  skip-not-fail — TestNG behavior). Edges gate on pass/fail only; **data flows through fixtures**,
  not deps. Independent leaves run concurrently up to `concurrency`; an edge orders regardless of
  job count. *(`proofs/ordering/depends_on_test.lua`: login→populate→journeys + transitive group-edge
  cascade; `dag_serial` proves a chain serializes under `concurrency = 8`.)*
- **Resources + the concurrency scheduler**: typed constructors `prova.port(n)` /
  `prova.resource(tok)` (exclusive) and `prova.shared(x)` (concurrent reader), plus bare-string
  tokens (exclusive) and `{ serial = true }` (process-wide exclusive). Each leaf carries `reqs`
  (own + inherited group resources); the scheduler holds a **readers-writer** `ResourceTable` and
  launches a leaf only when its deps passed **and** its reqs are acquirable (reader waits for a
  writer; writer waits for all). Acquisition is all-or-nothing per leaf, so no hold-and-wait → no
  deadlock. `serial` is desugared to an exclusive hold on a reserved global token that every other
  leaf reads (injected only when some leaf is serial). Declarations are **inert at `concurrency =
  1`** and enforced above it — so raising `--jobs` is the throughput-only, surprise-free knob the
  design promises. *(`proofs/resources/scheduler_test.lua`; `resources` tests prove exclusive holders
  serialize (~80ms) while shared readers overlap (~40ms) under `concurrency = 8`.)* CLI: `--jobs N`
  / `-j N`.
- **Assertions**: `t:expect(subject, label?)` → matchers `equals`/`eq` (**deep** for tables), `is`
  (**identity** — same reference / `rawequal`, for "the same object" incl. tables with function fields
  deep-equals can't compare), `is_true`/`is_false`/`is_nil`/`is_truthy`/`is_falsy`, `contains`,
  `matches` (Lua pattern),
  `has_length`, `is_one_of`, `gt`/`gte`/`lt`/`lte`, and filesystem `exists`/`is_file`/`is_dir`/
  `is_empty`, plus **`is_fully_rendered`** — the signature archetype check: scans every file under a
  rendered tree (contents + path segments) for leftover jinja markers (`{{`/`{%`/`{#`), excluding
  GitHub `${{ … }}`, and fails listing the offenders (path-string or handle-table subject); `:never()`
  negates; optional `label`. **Soft
  assertions** via `t:expect_all(fn)` — collect every failure in the block and fail once with all of
  them (not just the first). Plus `t:skip`, `t:log`. *(`testdata/assertions*.lua`.)*
- Concurrent async execution (proven) + I/O timeouts via cancellation + a readers-writer
  **resource** scheduler (`prova.port`/`resource`/`shared`, `serial`) making `--jobs > 1` safe.
  Default execution stays **sequential** (`concurrency = 1`); resource declarations are inert there.
- **Multi-file suite runner** (`suite.rs`): `discover_files` finds `*_test.lua` / `*.test.lua`;
  `run_suite` runs them across a pool of **per-worker Lua states** (true multi-core across files) —
  one file, or `--jobs 1`, stays inline single-state. Workers stream owned node events back to a
  single coordinator/reporter. Known limitation: a cross-file `suite` fixture is per-worker under
  `--jobs > 1` (a Lua value can't cross `!Send` states; a serialized once-guard is future work).
- `Event`/`Reporter`/`MultiReporter`/`JsonReporter`; `discover_path`; CLI takes files **or
  directories**, `--list` / `--format json` / `--jobs N`.
- **Package manifest** (`prova.toml`) + **CI**: `prova` with no args runs the suite declared in
  `prova.toml` (`[run]` = default profile; `[profiles.<name>]` overlays via `--profile`); a profile
  sets `proofs`/`jobs`/`format`/`env` (full schema: [`manifest.md`](manifest.md)). CLI flags
  override the manifest; explicit path args bypass it.
  The `env` table is applied before the run, so the *same* suite targets ephemeral containers
  locally or CI-provided/live services just by switching profile. A composite GitHub Action
  ([`prova-rs/run-action`](https://github.com/prova-rs/run-action)) installs a released binary and
  runs the suite in CI. *(Manifest parse/resolve unit-tested in `prova-cli`.)*
- **Self-tests (dogfooding)**: prova acceptance-tests **itself** — `crates/prova-cli/selftest/`
  `*_test.lua` invoke the real `prova` binary (via `shell`) against inner fixtures and assert on exit
  codes + output (tally, `--list`, `--format json`, error paths, manifest profiles + env). Driven by
  `tests/selftest.rs` (`prova` → runs `*_test.lua` → each shells to `prova` → asserts). This is
  black-box coverage of the assembled CLI the library tests can't reach — and it already caught a
  real bug (the CLI advertised `--format json` but only parsed `--format=json`; fixed).

The scheduler/lifecycle **spine is now complete** (collect → plan → deps → resources → multi-core
execute). The remaining increments pivot from engine to **product** — the capabilities that make
prova useful beyond testing itself:

1. **Resource modules** — Redis (`cache`), Kafka/Pulsar (`messaging`), S3/Azure-blob (object storage):
   the remaining archetype resource types, as `docker`-provisioned ephemeral deps + thin client
   modules. *(The network-interface trio `http`/`grpc`/`graphql` is done; server databases are
   plugins — `require("postgres")`/`require("mysql")` over the extracted `db` core, `sqlite` stays
   built in (see [`namespacing.md`](namespacing.md)); `net.free_port` and `http.client` landed.)*
2. **Snapshots** — `matches_snapshot`, `.snap` files + `--update-snapshots`.
5. **Flow ergonomics — resolved.** `test_each` + `describe` shipped. `f:use(fixture)` builder sugar
   and parametrized fixtures (`ctx:param`) were both **dropped** as magic that fights the explicit,
   lazy-`ctx:use` model; flow-scoped fixtures use `t:use` inside steps (scope-cached → same instance
   across steps). Re-running the flow *builder* (its only real consumer would be a load executor,
   which is a non-goal) is not planned — see the north-star roadmap.
6. **Selectors** (tag expressions, `--last-failed`, sharding), richer reporters (JUnit/TAP), and the
   **load executor**.
7. **Cross-worker `suite` fixtures**: a serialized once-guard for serializable values (the one open
   semantic from the multi-core step).

### North Star (the acceptance scenario the whole design serves)

Generate a Rust gRPC service (Postgres + Pulsar producer) and a Go REST service (MySQL + Pulsar
consumer) from archetypes; provision the DBs, a Pulsar cluster/topic, and dynamic ports as ephemeral
containers; boot both apps wired to those endpoints; then drive gRPC + REST and query both databases
to assert cross-service state. Every ingredient is a module behind the plugin boundary — `archetect`
(generate, or generate scaffolding on the fly), `docker` (ephemeral deps), `shell.spawn` (boot),
`http`/`grpc` (drive), the `postgres`/`mysql` plugins (assert state) — composed by fixtures and
gated by `requires`. The same
suite can instead point at a **dev Kubernetes cluster** (skip the containers, set endpoints via a
manifest profile's `env`) — local, CI, and environment testing from one description.

**Single-service assembly is proven** (the `service_grpc_postgres` capstone): a real p6m
`rust-grpc-service-archetype@dev` rendered with Postgres, built, booted against a provisioned Postgres,
and driven over gRPC (`grpc.call_status`) while a Postgres client cross-checks the same database —
31.8s, green, leak-free. It also demonstrates prova's forcing-function value: *running* the service
exposed that the archetype is a scaffold (methods `Unimplemented`, empty migration) — something
"renders + compiles" hides. (This capstone predates the resource-client extraction; it is being
updated to `require("postgres")` + a `prova.toml` plugin declaration — the new external-plugin model.)
The remaining gap to the full North Star is the second service + a stream + cross-service assertions,
which are more of the same composition.
