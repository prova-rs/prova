# Prova — Runtime Architecture

> Companion to [`api.md`](api.md) (authoring surface) and [`foundations.md`](foundations.md)
> (the thesis). This doc is the *engine*: async model, the definition→plan→execute pipeline,
> output as a plugin surface, and the frontend protocol that lets a GUI/IDE drive the same core
> the CLI does. Status: **early implementation** (`crates/prova-core`). Not all of this is built
> yet; what is built is noted, and everything here is a decision the built parts already respect.

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
- **Per-worker states for multi-core (built).** True CPU parallelism needs more than one thread,
  and an `mlua::Lua` is best kept to one thread — and its `Function` bodies are `!Send`, so a body
  collected on one state cannot run on another. The realized design: **N workers = N OS threads,
  each with its own Lua state**, with the **file** as the dispatched unit — a worker loads a file
  into its own state and runs it end to end with the in-file scheduler; within a worker, cooperative
  async as above. Files run in parallel; `--jobs` sets the worker count and is **throughput-only,
  never semantic**. *(Built in `suite.rs`; proven: two CPU-bound files run ~1.8× faster at `--jobs
  2` than `--jobs 1`.)* Intra-file unit dispatch across workers is a possible future refinement, but
  the file boundary is the clean one under `!Send`.
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
  `NodeFinished{path, outcome, duration, assertions, message}`, `RunFinished{summary}`. The
  executor only *emits*; it never prints.
- **`Reporter`** — one method, `event(&Event)`. Implementations: `ConsoleReporter` (human),
  `JsonReporter` (JSONL wire protocol), `NullReporter` (tests / load driver).
- **`MultiReporter`** — fan-out, so console + a JUnit writer + a GUI socket can run simultaneously.
- Planned sinks: JUnit XML, TAP, and a load-metrics aggregator (consumes the same stream, emits
  latency histograms/percentiles instead of pass/fail lines).

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
| **Modules** | async Lua modules registered into the runtime (via `RunConfig::with_module`) | `fs`, `shell`, `http`, `grpc`, `docker`, `db`, `archetect`, later `graphql` |
| **Matchers** | new terminal checks on the matcher | domain assertions, snapshots |
| **Reporters** | new `Reporter` sinks | JUnit, TAP, GUI socket, load metrics |
| **Selectors** | plan filters | tag expressions, `--changed`, `--last-failed`, sharding |
| **Executors** | alternate drivers over the plan | acceptance (once), **load/stress** (many, sustained) |

**`docker`** — ephemeral dependencies, testcontainers-style — is **built** (`docker.run` → a
`Container`, the same managed-lifecycle pattern `shell.spawn`'s `Process` established), paired with
`requires` gating so a suite skips where Docker is absent. **`grpc`** is now built too — a native,
dynamic, reflection-driven client (no `grpcurl`, no `.proto` files), the same async-module shape as
`http` (call a method, assert on the response/status). Still planned:

- **`graphql`** — the last network interface alongside `http`/`grpc`. Same async-module shape.

The end-to-end arc the modules serve is now real through the network-drive step: **render the
project → assert the layout → boot the app (`shell.spawn`) → spin up its dependencies (`docker`) →
drive its network interfaces (`http`/`grpc`; `graphql` next) → tear it all down** — one framework,
no capability ceilings.

The **load/stress** executor is the clearest payoff of these seams: a `flow` is already a reusable
scenario; a load driver takes that scenario, runs it under a concurrency/duration/arrival profile
(the async spine makes thousands of in-flight iterations cheap), and feeds the same event stream
into a metrics reporter. No new authoring surface — the same tests, driven differently.

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
  `ctx:use(handle|name)` builds-or-caches; `test`/`flow`/`file`/`suite` scopes with per-scope
  caches; `ctx:defer` (LIFO); `ctx:tempdir` (auto-removed); scope-mismatch rejection; inner→outer
  teardown. **`ctx:use` is async** — a factory can `await` (e.g. `shell.run`, a readiness poll);
  recursion reenters through Lua, so no boxing. **Teardown is async too** — a `ctx:defer` callback
  can `await` (e.g. `proc:stop()`), reaped while the runtime is still alive. *(`lifecycle_poc_test`;
  `async_fixture`; `service_lifecycle_test` boots a process and stops it on teardown, leak-free.)*
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
  *(`examples/shell_fs_test.lua`; `examples/http_probe_test.lua` boots a server + probes it.)*
  `http` is feature-gated (default on) and HTTP-only in v1 — an `https`/rustls feature layers on
  later; the rest of the stack needs no TLS. Also **`docker`** (`docker.run{image, ports, env,
  wait}` → a `Container`: `.id`, `:host_port(p)`/`:endpoint(p)`, async `:logs()`/`:exec(cmd)`/
  `:stop()`) — testcontainers-style ephemeral deps via the typed **bollard** daemon client (not CLI
  parsing): pull, create + start with random host-port bindings, inspect for the mapped ports,
  readiness wait (port TCP-connect or log-substring), `remove_container(force)` on `:stop()` with a
  `Drop` backstop so a container never leaks. *(`examples/docker_dependency_test.lua`; verified
  against a real daemon — whoami HTTP, redis exec/logs, real Postgres.)*
- **`db` module** — one **general, multi-database** query API over **sqlx's `Any` driver**:
  `db.connect(url)` picks the backend by URL scheme (`postgres://`, `mysql://`,
  `sqlite://…?mode=rwc`), returning a `Connection` with async `:execute` (rows affected),
  `:query` (list of column-name→value tables, NULL→nil, typed by SQL kind with a probe fallback for
  computed columns like `count(*)`), `:query_value` (scalar), `:close`. Positional params bind
  Lua int/float/bool/string/nil. Feature-gated `db` (default on). *(`tests/db.rs` verifies the full
  surface over SQLite;* ***`examples/db_postgres_test.lua` + `tests/db_postgres.rs` run the identical
  API against a real Postgres in an ephemeral `docker.run{postgres}` container*** *— the North Star
  data layer, gated by `requires` so it skips without a daemon.)*
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
  single-self-contained-binary promise. *(`examples/grpc_test.lua` + `tests/grpc.rs` run the three
  round-trips — unary, field echo, and a `NotFound` status — against a real reflection-enabled server
  (`moul/grpcbin`) in an ephemeral container, gated by `requires` so it skips without a daemon.)*
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
  *(`examples/flow_poc_test.lua` runs green: shared upvalue, shared flow-fixture, cascade proven.)*
- **Dependency DAG** (`depends_on`): `prova.test`/`flow`/`group` return `UnitHandle`s. `build_plan`
  flattens the tree into leaves (tests + flows; a group is not a leaf) and expands each unit's
  `depends_on` — folding in **inherited** group-level deps — into concrete leaf edges (a dep on a
  group fans out to that group's leaves). Cycles are a collection-time error (defensive; Lua's
  backward handle refs make them practically unreachable). The scheduler runs a leaf once all its
  dependency leaves have **passed**; any failed/skipped dep **cascade-skips** it (transitively,
  skip-not-fail — TestNG behavior). Edges gate on pass/fail only; **data flows through fixtures**,
  not deps. Independent leaves run concurrently up to `concurrency`; an edge orders regardless of
  job count. *(`examples/depends_on_test.lua`: login→populate→journeys + transitive group-edge
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
  design promises. *(`examples/resources_test.lua`; `resources` tests prove exclusive holders
  serialize (~80ms) while shared readers overlap (~40ms) under `concurrency = 8`.)* CLI: `--jobs N`
  / `-j N`.
- **Assertions**: `t:expect(subject, label?)` → matchers `equals`/`eq` (**deep** for tables),
  `is_true`/`is_false`/`is_nil`/`is_truthy`/`is_falsy`, `contains`, `matches` (Lua pattern),
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
- **Suite manifest** (`prova.toml`) + **CI**: `prova` with no args runs the suite declared in
  `prova.toml` (`[run]` = default profile; `[profiles.<name>]` overlays via `--profile`); a profile
  sets `paths`/`jobs`/`format`/`env`. CLI flags override the manifest; explicit path args bypass it.
  The `env` table is applied before the run, so the *same* suite targets ephemeral containers
  locally or CI-provided/live services just by switching profile. A composite GitHub Action
  (`ci/action.yml`) + example workflow (`ci/example-workflow.yml`) run it in CI. *(Manifest
  parse/resolve unit-tested in `prova-cli`.)*
- **Self-tests (dogfooding)**: prova acceptance-tests **itself** — `crates/prova-cli/selftest/`
  `*_test.lua` invoke the real `prova` binary (via `shell`) against inner fixtures and assert on exit
  codes + output (tally, `--list`, `--format json`, error paths, manifest profiles + env). Driven by
  `tests/selftest.rs` (`prova` → runs `*_test.lua` → each shells to `prova` → asserts). This is
  black-box coverage of the assembled CLI the library tests can't reach — and it already caught a
  real bug (the CLI advertised `--format json` but only parsed `--format=json`; fixed).

The scheduler/lifecycle **spine is now complete** (collect → plan → deps → resources → multi-core
execute). The remaining increments pivot from engine to **product** — the capabilities that make
prova useful beyond testing itself:

1. **`graphql` module** — the last network interface. *(The `grpc` module — native/dynamic via
   reflection + `prost-reflect`, verified against a real reflection server — is done, as is the `db`
   module (sqlx `Any`, real Postgres) and the `bollard` swap for Docker.)*
2. **Snapshots** — `matches_snapshot`, `.snap` files + `--update-snapshots`.
5. **Flow ergonomics**: `f:use(fixture)` builder sugar (currently flow-scoped fixtures are used via
   `t:use` inside steps); re-runnable flow bodies (re-invoke to get fresh closures) as the
   precondition for the **load executor** treating a flow as a reusable scenario; `test_each` +
   parametrized fixtures (`ctx:param`).
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
`http`/`grpc` (drive), `db` (assert state) — composed by fixtures and gated by `requires`. The same
suite can instead point at a **dev Kubernetes cluster** (skip the containers, set endpoints via a
manifest profile's `env`) — local, CI, and environment testing from one description.

**Single-service assembly is proven** (`examples/service_grpc_postgres_test.lua`): a real p6m
`rust-grpc-service-archetype@dev` rendered with Postgres, built, booted against a `docker.run` Postgres,
and driven over gRPC (`grpc.call_status`) while `db.connect` cross-checks the same database — 31.8s,
green, leak-free. It also demonstrates prova's forcing-function value: *running* the service exposed
that the archetype is a scaffold (methods `Unimplemented`, empty migration) — something "renders +
compiles" hides. The remaining gap to the full North Star is the second service + Pulsar + cross-service
assertions, which are more of the same composition.
