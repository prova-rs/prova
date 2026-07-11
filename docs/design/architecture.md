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
| **Modules** | async Lua modules registered into the runtime (via `RunConfig::with_module`) | `fs`, `shell`, `http`, `archetect`, later `grpc`, `container`, `db` |
| **Matchers** | new terminal checks on the matcher | domain assertions, snapshots |
| **Reporters** | new `Reporter` sinks | JUnit, TAP, GUI socket, load metrics |
| **Selectors** | plan filters | tag expressions, `--changed`, `--last-failed`, sharding |
| **Executors** | alternate drivers over the plan | acceptance (once), **load/stress** (many, sustained) |

A **`grpc` module** is a strong candidate next module: it fits the exact async-module shape `http`
already uses (call a method, assert on the response), and archetect-core already carries a gRPC
proto + fixtures to lean on. Reflection-based or `.proto`-driven method invocation with
status/message matchers would extend prova into service-contract testing without touching the core.

The **load/stress** executor is the clearest payoff of these seams: a `flow` is already a reusable
scenario; a load driver takes that scenario, runs it under a concurrency/duration/arrival profile
(the async spine makes thousands of in-flight iterations cheap), and feeds the same event stream
into a metrics reporter. No new authoring surface — the same tests, driven differently.

## Current status (implemented)

- Async collect→plan→execute for `prova.test` / `prova.group` / `prova.flow`; injected `prova`
  global. All three (and the builder variants) accept an optional `opts` table and return unit
  handles for `depends_on`.
- **Fixtures + scopes + teardown**: `prova.fixture(name, scope, factory)` → typed handle;
  `ctx:use(handle|name)` builds-or-caches; `test`/`flow`/`file`/`suite` scopes with per-scope
  caches; `ctx:defer` (LIFO); `ctx:tempdir` (auto-removed); scope-mismatch rejection; inner→outer
  teardown. **`ctx:use` is async** — a factory can `await` (e.g. `shell.run`, a readiness poll);
  recursion reenters through Lua, so no boxing. *(`lifecycle_poc_test.lua`; `async_fixture` proves a
  factory that awaits and chains through a fixture-uses-fixture edge.)*
- **Capability modules** (`modules.rs`), injected as their own globals: **`shell.run(cmd, {cwd,
  env, timeout, check})`** (async via `tokio::process`; returns `{code, stdout, stderr, duration}` +
  `:ok()`); **`fs`** (`exists`/`read`/`write`/`remove_all`/`tempdir`/`glob`); and **`http`**
  (`get`/`post`/`put`/`delete`/`wait_for`; async via reqwest; response `.status`/`.body`/`.headers`
  + `:json()`; `wait_for` is the boot-then-probe poll). Filesystem matchers
  `:exists()`/`:is_file()`/`:is_dir()` take a path-string **or handle-table** subject. This is the
  slice that lets prova test a real rendered workspace and a running service.
  *(`examples/shell_fs_test.lua`; `examples/http_probe_test.lua` boots a server + probes it.)*
  `http` is feature-gated (default on) and HTTP-only in v1 — an `https`/rustls feature layers on
  later; the rest of the stack needs no TLS.
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
  `is_empty` (path-string or handle-table subject); `:never()` negates; optional `label`. **Soft
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

The scheduler/lifecycle **spine is now complete** (collect → plan → deps → resources → multi-core
execute). The remaining increments pivot from engine to **product** — the capabilities that make
prova useful beyond testing itself:

1. **Snapshots + gating** — snapshot assertions (`matches_snapshot`, `.snap` files +
   `--update-snapshots`) and `requires` (capability gating → skip, not fail). *(The matcher surface +
   soft `expect_all` — done.)*
2. **Flow ergonomics**: `f:use(fixture)` builder sugar (currently flow-scoped fixtures are used via
   `t:use` inside steps); re-runnable flow bodies (re-invoke to get fresh closures) as the
   precondition for the **load executor** treating a flow as a reusable scenario.
3. **Selectors** (tag expressions, `--last-failed`, sharding), richer reporters (JUnit/TAP), and the
   **load executor**.
4. **Cross-worker `suite` fixtures**: a serialized once-guard for serializable values (the one open
   semantic from the multi-core step).
