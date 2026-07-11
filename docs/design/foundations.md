# Prova — Foundations: what it takes to *not* be another half-baked test framework

> Companion to [`api.md`](api.md). That doc specifies the authoring surface; this one is the
> thesis behind it — the primitives and footguns that decide whether Prova becomes the
> one-stop acceptance-testing tool or just adds to the pile.

## Thesis: orthogonal primitives, not a feature pile

You do not subsume the testing landscape by implementing every framework's feature list.
You subsume it by **factoring testing into a small set of orthogonal primitives that
compose**. Every framework that only half-solves testing does so because it *hard-codes one
axis* and then cannot cleanly express the others.

| Framework      | Nails                                   | Hard-codes / weak at                          |
|----------------|-----------------------------------------|-----------------------------------------------|
| pytest         | fixtures (lifecycle), markers           | ordering/deps are a plugin; parallelism not resource-aware |
| TestNG         | groups + `dependsOn` + data providers   | fixture composition; JVM-bound                |
| Go `testing`   | hermetic, parallel, single binary, seed | intentionally minimal (no fixtures/deps/params)|
| Jest/Playwright| DX (watch/`.only`), tracing, `serial`   | JS-bound; weak dependency graph               |
| Cucumber/Robot | living docs / BDD                       | programmability wall                          |

The union of what they do well decomposes into **five orthogonal axes** plus cross-cutting
concerns. Get the axes right and independent, and the rest is expressible.

### The five axes

1. **Lifecycle (scope)** — when state is constructed and destroyed. *(Prova: fixtures with
   `test`/`file`/`suite` scopes.)*
2. **Selection (tags → expressions)** — which tests run. A boolean tag-expression language,
   not single tags.
3. **Ordering / Dependency (a DAG)** — explicit sequence and *skip-dependents-when-upstream-
   fails*. This is the TestNG axis and the thing acceptance/integration testing actually
   needs.
4. **Concurrency (declared resources)** — what may run in parallel without corrupting shared
   state (ports, DBs, files). The scheduler enforces it.
5. **Provisioning (fixtures + SUT lifecycle)** — how the system-under-test is brought into
   existence (render, build, spawn, containers) and torn down.

### Cross-cutting (apply to all axes)

- **Reporting & observability** — machine formats (JUnit XML, TAP, JSON) + rich native
  output, per-test captured logs/stdout/attachments, value-bearing assertion diffs.
- **Environments & config** — run the same suite against dev/staging/prod targets.
- **Determinism** — seeded randomization, reproducible failing runs.
- **Extensibility** — a plugin surface (Lua-first, Rust for protocols) so the long tail is
  filled without forking. *This is how pytest actually won — not features, plugins.*

**Design rule:** these are separate dimensions. Scope ≠ tag ≠ dependency ≠ resource. A test
has a scope for its fixtures, tags for selection, optional dependency edges, and declared
resources — independently. Frameworks get muddy when they conflate them (e.g., using a
"group" for both selection *and* lifecycle *and* ordering).

---

## The axes in detail

### 2. Selection — tag expressions, inherited, dynamic

Single tags are not enough; the winning capability is a **boolean expression language** over
tags (JUnit5 tag expressions, pytest `-m "slow and not network"`):

```
prova test -m "acceptance and not slow"
prova test -m "(smoke or regression) and not offline"
```

- **Tags attach at every level** (suite/file/`describe`/test) and **inherit downward**: tag
  a `describe` "slow" and every test inside is slow.
- **Built-in dynamic selectors** that plugins-elsewhere charge for:
  - `--last-failed` / `--failed-first` — rerun/prioritize the previous failures (pytest `--lf/--ff`).
  - `--changed` — only tests whose inputs changed (test-impact analysis) — huge for big suites.
  - `--shard k/n` — deterministic CI sharding.
  - `--only`/`.only` and `.skip` inline focus for the local loop.
- **Reserved capability tags** wired to real checks: `offline`, `docker`, `network` — see
  capability-gating under Footguns.

### 3. Ordering & dependency — isolated by default, ordered flows by choice

The honest position separates two knobs most frameworks conflate: **isolation** (do tests
share state?) and **order** (what sequence runs?). Prova's answer is to make **execution
strategy a property of the container**, not a global default or a CLI flag: `prova.group`
(independent — isolated, unordered, parallelizable) vs. `prova.flow` (sequence — ordered
steps sharing context, cascade-skip). You read the container and you know. This applies
*make-invalid-states-unrepresentable* to execution — shared mutable context is a flow-only
capability a `group` never receives — and it makes `--jobs` purely a throughput knob, never
semantic. We do *not* randomize by default; randomization is an **opt-in hardening pass**
(`--shuffle[=seed]`) that proves a group's children are truly independent. TestNG gained
traction precisely because it made ordered, dependent flows first-class where pytest fumbles
them; we make both strategies first-class and visible. See `api.md` → "Execution model" for
the full statement. The two ordering constructs:

**Dependency edges (hard):** a test declares upstream dependencies; the runner topo-sorts,
and if an upstream *fails*, dependents are **skipped, not failed** (the key TestNG behavior):

```lua
local created = prova.test("creates the resource", function(t) ... end)
prova.test("reads it back", { depends_on = { created } }, function(t) ... end)  -- skipped if create failed
```

**Flows / scenarios (ordered + shared context):** a first-class construct for stateful
sequences. Steps run in order, **share one context object**, and later steps auto-skip once
an earlier step fails. This is the "built-up context" integration pattern, made safe:

```lua
prova.flow("order lifecycle", function(flow)
  local order_id
  flow:step("create order", function(t)
    order_id = http.post(api .. "/orders", { json = {...} }):json().id
    t:expect(order_id):is_truthy()
  end)
  flow:step("read the order", function(t)          -- runs only if "create" passed
    t:expect(http.get(api .. "/orders/" .. order_id).status):equals(200)
  end)
  flow:step("delete the order", function(t) ... end)
end)
```

Flows are the sanctioned home for shared mutable state — *not* an abuse of `suite`-scoped
fixtures. Isolated tests stay isolated and randomizable; flows opt into ordering locally.

### 4. Concurrency — declared resources, not hope

Parallelism corrupts acceptance tests through shared *external* state (a port, a DB schema,
a filesystem path, an account). The fix is **declared resources** that gate the scheduler
(Playwright's serial groups and Cargo-nextest's think along these lines, but as a
first-class primitive here):

```lua
prova.test("binds :8080", { resources = { "port:8080" } }, function(t) ... end)      -- exclusive
prova.test("reads shared db", { resources = { prova.shared("db") } }, function(t) ... end) -- shared-read
```

- A resource is exclusive by default; `prova.shared(x)` allows concurrent readers.
- The scheduler is a constraint solver over resources → maximal safe parallelism with zero
  races, *without* forcing `--jobs 1`.
- `file`/`suite`-scoped fixtures interact with this: a `file`-scoped fixture pins its file's
  tests to one worker (its cached value can't cross a process boundary cheaply); a `suite`
  fixture needs a **once-guard** so parallel workers don't double-build it. This is a known
  hard corner — see Footguns.

### 5. Provisioning — SUT lifecycle is a first-class citizen

The thing acceptance testing uniquely needs and unit frameworks ignore: **bringing the
system-under-test into existence and tearing it down reliably.** Prova treats this as core,
via provisioning modules behind the fixture boundary:

- **process** — `shell.spawn` returning a handle with health-wait and **process-group kill**
  on teardown (see the `&` footgun). Boot a server, wait, probe, guaranteed cleanup.
- **container** — a testcontainers-style module (`container.run{ image=..., ports=... }`) —
  the single most-requested integration primitive of the last decade.
- **compose / stack** — bring up a multi-service topology, tear down as a unit.
- **mock** — a WireMock-style stub server fixture for external dependencies.

These are first-party *plugins*, not built-ins — same boundary the `archetect` render module
sits behind — so the agnostic core never grows a Docker dependency.

---

## Classic footguns (and the decision that defuses each)

| Footgun | Why it bites | Prova's foundation |
|---|---|---|
| **Teardown skipped on failure/panic/timeout** | Leaked containers, ports, temp dirs poison later runs | Teardown is guaranteed: `defer`s run on pass, fail, assertion-abort, *and* test timeout; best-effort on signals. Teardown errors are reported, never swallowed. |
| **`shell.run("server &")` orphans processes** | Backgrounded child escapes; port stays bound | No backgrounding. `shell.spawn` returns a handle; teardown kills the whole **process group** (not just the pid). |
| **Hidden inter-test coupling** | Passes in order, fails when reordered/parallelized | **Isolation by construction** (no ambient global state to leak) prevents most coupling structurally; opt-in `--shuffle[=seed]` hardening surfaces anything that slips through, reproducibly. |
| **Retries hide real flakiness** | Green build masks a genuine race | Retries allowed but **flakes are surfaced**: a test that passes only on retry is reported `FLAKY`, countable and quarantinable — not silently green. |
| **Scope mismatch** | A `suite` fixture depends on a `test` fixture → nonsense lifetime | **Validated at collection**: a broader-scoped fixture may not depend on a narrower one; hard error with a clear message (pytest does this; we do it up front). |
| **Parallel double-build of a `suite` fixture** | Two workers each construct the "once" resource | Once-guard keyed by `(fixture, run)` with cross-worker coordination; documented ownership model. |
| **Nondeterministic failures irreproducible** | "Works on my machine," can't debug CI | Every source of nondeterminism (order, property-test inputs) is **seeded and the seed is printed**; `--seed X` reproduces exactly. |
| **Snapshot rot** | Stale/over-broad snapshots rubber-stamp bugs | Snapshots are reviewable diffs; `--update-snapshots` is explicit; unused snapshots are flagged. |
| **Environment/cwd/env-var pollution** | Test mutates global state, next test inherits it | **Hermetic by default**: per-test temp cwd, scoped env, temp HOME. Mutations are sandboxed and reverted. |
| **"Test passed" but nothing asserted** | Empty or all-skipped body reports green | Warn on tests with **zero assertions**; distinguish `skipped`/`xfail` from `passed` in every reporter. |
| **Capability-missing = failure, not skip** | No Docker/offline → red build instead of "n/a" | **Capability gating**: `requires = { "docker" }` / `{ "network" }` → auto-**skip with reason** when the capability is absent, and selectable via tags. |
| **Slow suite, no signal** | Devs stop running it | First-class timing per test/fixture, `--durations` slowest-N, and a fixture-setup-vs-test timeline in the report. |

---

## Advanced features to design *seams* for now (even if unimplemented)

Building the seam early is cheap; retrofitting is not.

- **Property-based testing** — generators with shrinking (Hypothesis/QuickCheck). Seam: a
  parametrization source that yields generated cases + a shrink protocol. Reuses axis 2's
  seeded determinism.
- **BDD / living docs (optional layer)** — a thin Gherkin-ish surface that *compiles to
  flows*, for orgs that want given/when/then and stakeholder-readable reports. Optional skin
  over the core, never the core itself — this is how you absorb the Cucumber/Robot audience
  without becoming them.
- **Test-impact analysis** (`--changed`) — needs an input-fingerprint per test (sources,
  fixtures, data). Design the fingerprint hook now.
- **Watch mode** — re-run affected on file change; leans on the same impact graph.
- **Distributed / sharded execution** — `--shard k/n` now; remote workers later. Keep the
  scheduler pure over the resource model so distribution is a transport swap.
- **Rich attachments** — per-test artifacts (rendered tree, HTTP exchange, screenshots,
  container logs) surfaced in reports (Allure/Playwright-trace style). Acceptance testing
  lives or dies on "show me what actually happened."
- **Requirement traceability** — map tests → acceptance criteria/IDs for coverage reports.

---

## The boundary (what we deliberately do *not* subsume)

Being comprehensive at acceptance testing requires being honest about where we stop —
otherwise we're half-baked at everything.

- **In-language unit testing** stays with JUnit/pytest/Go/Vitest. We cannot mock a Java
  private method or introspect a Go struct from Lua, and pretending otherwise is the trap.
  Prova is **black-box / out-of-process**: it tests systems through their real surfaces
  (files, processes, HTTP/gRPC, exit codes), not a language's internals.
- **Load/performance testing** stays with k6/Gatling. We assert correctness of behavior, not
  throughput distributions. (We may *measure* timing, not *model* load.)

Owning the acceptance/integration layer completely is a bigger, less-served prize than
fighting JUnit on its home turf. The line is: **if the assertion needs to reach inside the
process's memory, it's out of scope; if it observes the system from outside, it's ours.**

---

## Minimal core vs. plugins (how "subsume" stays buildable)

`prova-core` implements only the axes and cross-cutting engine:

- collection, tag-expression selection, the dependency DAG + flows, the resource scheduler,
  the fixture/scope/teardown machine, `expect`, reporters (pluggable), config/environments,
  seeded determinism, the plugin/hook API.

Everything domain-specific is a **plugin** (Lua-first for matchers/reporters/fixtures;
Rust for protocols/perf): `fs`, `shell`/`process`, `http`, `container`, `compose`, `mock`,
`grpc`, and `archetect`. New capability = new plugin, never a core fork. That plugin surface
— not a bigger feature list — is what lets Prova grow to cover the landscape the way
pytest's plugin ecosystem did, while the core stays small and correct.

---

## Open decisions this raises (feed back into `api.md`)

1. **Dependency + flow syntax** — `depends_on` handles vs. named refs; `flow:step` shape and
   whether steps can themselves be tagged/skipped individually.
2. **Resource model surface** — string tokens (`"port:8080"`) vs. typed resource objects;
   how `shared` vs. exclusive is expressed; interaction with `file`/`suite` fixtures.
3. ~~Randomize-by-default~~ **Decided**: deterministic definition order + isolation by
   construction; `--shuffle[=seed]` is opt-in hardening, not the default. (See api.md →
   Execution model.)
4. **Capability registry** — how `requires = {"docker"}` resolves to a probe; are capabilities
   first-party-only or plugin-registerable?
5. **Hermeticism depth** — how far the per-test sandbox goes (cwd/env always; temp HOME opt-in?).
6. **Plugin API shape** — the hook points (collection, before/after each scope, reporter,
   selector, matcher) and whether v1 exposes Lua plugins only or Rust too.
