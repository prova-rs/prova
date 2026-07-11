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
- **Per-worker states for multi-core (later).** True CPU parallelism needs more than one thread,
  and an `mlua::Lua` is best kept to one thread. The plan: **N workers = N OS threads, each with its
  own Lua state**, units dispatched across workers; within a worker, cooperative async as above.
  This composes with the container model — a `flow` is one unit pinned to one worker; independent
  units distribute. `--jobs` sets worker/concurrency counts and is **throughput-only, never
  semantic**.
- **`!Send` is fine.** Bodies, contexts, and fixtures are `Rc`/`RefCell` (single-thread). We use
  `FuturesUnordered`/`buffer_unordered` (poll-in-place, no `spawn`), so nothing needs `Send`.
  Cross-worker sharing (a `suite` fixture) will be an explicit, serialized handoff, not implicit.

## definition → plan → execute

```
collect (run the .lua file)        →  Node arena (groups/tests; later flows/fixtures)
  build_plan (walk, apply strategy) →  Vec<PlanItem> (path, body, timeout, params, deps, resources)
    run_plan (executor)             →  drives bodies, emits Events, tallies Summary
```

The **plan** is where strategy is resolved: group = independent items (parallelizable), flow =
an ordered sub-plan on one worker, `depends_on` = edges that gate/order items, resources = the
scheduler's constraints. Keeping the plan a distinct artifact is what lets a **load executor** be
a drop-in alternative to the acceptance executor over the same items.

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
| **Modules** | async Lua modules registered into the runtime | `fs`, `shell`, `http`, `archetect`, later `grpc`, `container`, `db` |
| **Matchers** | new terminal checks on the matcher | domain assertions, snapshots |
| **Reporters** | new `Reporter` sinks | JUnit, TAP, GUI socket, load metrics |
| **Selectors** | plan filters | tag expressions, `--changed`, `--last-failed`, sharding |
| **Executors** | alternate drivers over the plan | acceptance (once), **load/stress** (many, sustained) |

The **load/stress** executor is the clearest payoff of these seams: a `flow` is already a reusable
scenario; a load driver takes that scenario, runs it under a concurrency/duration/arrival profile
(the async spine makes thousands of in-flight iterations cheap), and feeds the same event stream
into a metrics reporter. No new authoring surface — the same tests, driven differently.

## Current status (implemented)

- Async collect→plan→execute for `prova.test` / `prova.group`; injected `prova` global.
- **Fixtures + scopes + teardown**: `prova.fixture(name, scope, factory)` → typed handle;
  `ctx:use(handle|name)` builds-or-caches; `test`/`file`/`suite` scopes with per-scope caches;
  `ctx:defer` (LIFO); `ctx:tempdir` (auto-removed); scope-mismatch rejection; inner→outer teardown.
  *(`examples/lifecycle_poc_test.lua` runs green, teardown order verified.)*
- `t:expect` matchers (`equals`/`eq`/`is_true`/`is_false`/`is_nil`/`is_truthy`/`contains`,
  `:never()`, optional label), `t:skip`, `t:log`.
- Concurrent async execution (proven) + I/O timeouts via cancellation. Default execution is
  **sequential** (`concurrency = 1`) until the resource scheduler makes parallelism safe.
- `Event`/`Reporter`/`MultiReporter`/`JsonReporter`; `discover_path`; CLI `--list` / `--format json`.

## Next increments

1. **Flows** (`prova.flow`/`f:step`, shared context, cascade-skip; `flow` scope) — the `flow`
   cache level slots into the existing scope machine.
2. **Units + `depends_on`** DAG (skip-downstream); **resources** + the constraint-solving
   scheduler; then safe parallelism + per-worker Lua states.
3. **Async fixtures** (upgrade `ctx:use` to an async method so factories can `await`) and async
   **modules**: `fs`/`shell`/`http`; soft assertions (`expect_all`); snapshots.
4. **Selectors** (tag expressions, `--last-failed`, sharding), richer reporters (JUnit/TAP), and the
   **load executor**.
