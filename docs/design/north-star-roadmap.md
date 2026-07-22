# Prova — North Star Roadmap & Session Hand-off

> Companion to [`architecture.md`](architecture.md) (engine), [`api.md`](api.md) (authoring surface),
> and [`foundations.md`](foundations.md) (thesis). This doc is the **sequenced plan** to reach the
> North Star, plus what a fresh session needs to resume. Read the auto-memory `prova-test-framework`
> first (it has every commit + gotcha); then skim `architecture.md`'s "Current status" and this file.

## The concrete mission (why prova exists)

The real target is the **[p6m-archetypes](https://github.com/p6m-archetypes)** org — ~140 untested
Archetect **v3 (Lua)** archetypes: `{rust,java,golang,python,typescript,dotnet}` × `{grpc,rest,graphql}`
services, each with `persistence` (Postgres/MySQL), `cache` (Redis), and `messaging` (Kafka/Pulsar)
options, composed from a dozen remote library archetypes. There is already a **Python/pytest harness**
([`archetype-test-harness`](https://github.com/p6m-archetypes/archetype-test-harness)) doing the easy
tier declaratively (`tests/manifest.yaml`: expected files, no leftover `{{ }}`, yaml globs, build
steps). **Prova must be *wholly better*: match that ergonomics on the easy tier, and make the hard
tier — provision real deps, boot the generated service, drive its gRPC/REST/GraphQL, assert DB/state —
*possible* at all** (which the declarative harness structurally cannot do).

**Verified (2026-07-12):** prova renders a real p6m archetype (`rust-grpc-service-archetype@dev`, 44
files, 12 composed libs) **headlessly in-process** and asserts its layout + `is_fully_rendered`.
**Rule:** every archetype must render headlessly; a prompt with no answer and no default **errors out**
(archetect-core already enforces this — `"Headless mode: no answer or default for 'X'"` — and
prova-archetect inherits it; never a hang).

Each generated project even ships a **conditional `docker-compose.yml`** declaring exactly the runtime
resources its answers imply (postgres/mysql/redis/kafka/pulsar) — the hard tier's spec, handed to us.

**The pytest manifest is the ergonomic *floor*.** If a prova archetype test is more verbose than a
`manifest.yaml`, the easy tier has failed. Met by **`archetect.verify{...}`** (in prova-archetect,
Lua sugar over prova primitives): renders once headlessly and registers the standard checks —
`expected_files`/`absent_files` (layout), `is_fully_rendered`, `yaml_globs` (each glob matches ≥1 file
and every match parses, via the `yaml` module), and a `requires`-gated `build_steps` — matching the
manifest field-for-field in ~10 lines, but as real Lua you can extend (it returns the shared render
fixture so you can add custom tests / the runtime tier alongside). *Verified against the real
`rust-grpc-service-archetype@dev`: layout + fully-rendered + yaml-manifests-parse, ~12 lines.*

## The broader ambition (beyond archetypes)

p6m-archetypes is the impetus, **not the ceiling**. The goal is a general-purpose **black-box /
acceptance / integration** framework people reach for in a **cloud-oriented, polyglot world** — where
today they fall back to bash/Python/Go glue. Not a unit-test framework (pytest/JUnit win there); the
wedge is out-of-process, environment-level testing where the batteries (`docker`, `db`, `http`,
`grpc`, `yaml`, fixtures, resources, `requires`-gating) and the single-binary polyglot-agnostic
packaging are the differentiator. Every DX decision serves *easy stuff easy, hard stuff possible* for
that general audience.

## The North Star (the target scenario)

A batteries-included, native black-box acceptance harness that can:

1. Generate a **Rust gRPC** service (Postgres + Pulsar producer) from an archetype, on an ephemeral port.
2. Generate a **Go REST** service (MySQL + Pulsar consumer) from an archetype.
3. Provision the databases, a Pulsar cluster/topic, and dynamic ports as **ephemeral containers**.
4. Boot both apps wired to those ephemeral endpoints.
5. Drive **gRPC + REST** and **query both databases** to assert cross-service state after calls.

Variations from one manifest/profile: point at a **dev Kubernetes cluster** (skip containers, set
endpoints via `env`), or **generate test scaffolding on the fly** with `archetect`. Local, CI, and
environment testing from one description.

## Where we are (built + verified, all committed)

The **spine and most capabilities are done**. Twelve+ increments, each with proving tests:

- Engine: async collect→plan→execute; fixtures/scopes/teardown (async `ctx:use` + async teardown);
  flows (`prova.flow`/`f:step`, cascade-skip); dependency **DAG** (`depends_on`); readers-writer
  **resource** scheduler (`prova.port`/`resource`/`shared`, `serial`); **multi-file suite runner**
  with per-worker Lua states (true multi-core); `requires` capability gating (skip-not-fail).
- Assertions: full matcher surface (`equals` deep, `matches`, `is_one_of`, `gt/gte/lt/lte`,
  `has_length`, fs `exists/is_file/is_dir/is_empty`, …) + soft `expect_all`.
- Capability modules: **`shell`** (`run` + `spawn`→managed `Process`), **`fs`**, **`http`**
  (`get/post/put/delete/wait_for`, `:json()`), **`docker`** (typed **bollard** client — pull/run/
  port-map/logs/exec/stop, `requires`-gated), **`db`** (sqlx `Any` — one API over Postgres/MySQL/
  SQLite by URL scheme), and the **`archetect`** plugin (in-process render, `prova-archetect` crate).
- Product surface: `prova.toml` **suite manifest** (profiles + env) + composite **GitHub Action**;
  **self-tests** (prova acceptance-tests its own CLI — caught a real `--format` bug).
- **Verified against a live daemon (OrbStack):** whoami HTTP, redis exec/logs, and a **real Postgres
  round-trip** (`docker.run{postgres}` + `db.connect` + query) — the North Star data layer, leak-free.

**North Star arc status:** render ✅ · assert layout ✅ · boot app (`shell.spawn`) ✅ · provision deps
(`docker`) ✅ · drive HTTP ✅ · **query DB ✅** · **drive gRPC ✅** · **single-service assembly ✅** ·
**virtualize a dependency + assert the interaction (`mock`) ✅** · Pulsar ⛔ · cross-service assembly ⛔.

**The `mock` facet (2026-07-16)** — unplanned; it was never a roadmap item, and arrived as a
first-class request rather than a phase. `http.mock` / `grpc.mock`: the fourth facet
(`namespacing.md`), core-native, with **Lua reply handlers** (so no response-templating language),
a request journal asserted on by the ordinary matchers, and passthrough/record/replay on the same
object. It matters to *this* list for one reason: item 3 of its scope — asserting on the
**interaction itself** ("did we call it exactly once, with the right idempotency key") — is a
question no arc entry above could answer, because a real dependency only ever tells you *whether it
worked*. Deliberately narrow: for a service you own, running the real one is still the better
answer, and that is what the topology arc is for. Plan + status: `docs/plans/mocks.md`.

**CAPSTONE PROVEN (2026-07-12):** the hard tier, end-to-end through prova against a **real** archetype
(`examples/service_grpc_postgres_test.lua`): render `rust-grpc-service-archetype@dev` with
`persistence=PostgreSQL` → `docker.run` Postgres → `cargo build` (~15s warm) → `shell.spawn` the binary
wired via `APP_PERSISTENCE__URL`/`APP_SERVER__PORT` → `grpc.wait_for` → `grpc.call_status` the service
→ `db.connect` the *same* Postgres and assert migrations ran. **31.8s, green, leak-free** (async
`ctx:defer` stops the container + process). This is the tier the pytest manifest structurally cannot
express — and it **exposed that the archetype is a scaffold** (methods return `Unimplemented`, migration
empty): *prova running the service is what reveals "renders + compiles" was hiding a hollow service.*
As the archetype grows real CRUD the assertion tightens (`Unimplemented` → `Ok` → real row round-trip)
— the test is the executable spec. (Known: service port hardcoded 50551; a `net.free_port()` helper is
the clean fix.)

## Sequenced plan

### Phase 1 — Finish the interfaces & authoring ergonomics

**1. `grpc` module — DONE (native, not grpcurl).** The roadmap penciled in grpcurl (option a) as the
quick unblock, but building it in natively (option b) was chosen to preserve prova's
single-self-contained-binary promise — shelling to `grpcurl` would put a `requires`-shaped hole in
exactly the "batteries-included, no capability ceilings" pitch. Implemented in `modules.rs` `mod grpc`
(feature `grpc`, default-on, `prova-core` still builds `--no-default-features`):
- `grpc.connect(addr, {timeout})` → `Client`; performs **gRPC Server Reflection once** to build a
  `prost_reflect::DescriptorPool` for every advertised service (skips `grpc.reflection.*`).
- `client:call("pkg.Svc/Method", req_table)` → response table (raises on non-OK status);
  `client:call_status(...)` → `{ok, code, message, response}` for status-code assertions;
  `grpc.wait_for(addr, {timeout, every})` = boot-then-probe.
- **How it works:** Lua table → `serde_json::Value` → `DynamicMessage::deserialize(input_desc)`; a
  generic tonic **codec over `DynamicMessage`** (encoder prost-encodes, decoder merges into an empty
  message of the output descriptor) does the unary call via `tonic::client::Grpc::unary`; reply
  `DynamicMessage` → serde_json (`skip_default_fields(false)`) → Lua. Reflection **negotiates v1,
  falls back to v1alpha** (a macro generates the per-version list/file-fetch pair).
- **Deps (first-class in `prova-core`, versions tracked to archetect-core so the lockfile dedupes to a
  single build):** `tonic` 0.14 (`default-features=false`, `["channel"]`), `tonic-reflection` 0.14
  (`default-features=false`), `prost`/`prost-types` 0.14, `prost-reflect` 0.16 (`serde`). Plaintext-
  only in v1 (matching `http`). **Not transitive** — prova-core stays domain-agnostic (no archetect
  edge); the tonic/prost tree merely happened to already be compiled via `prova-archetect`.
- *Verified:* `proofs/grpc/grpc_test.lua` + `tests/grpc.rs` — three round-trips (unary SayHello, DummyUnary
  field echo, a `NotFound` via `call_status`) against a real reflection server (`moul/grpcbin`, which
  speaks **v1alpha** — exercises the fallback) in an ephemeral container, `requires{docker}`-gated so
  it skips cleanly without a daemon. Clippy + LuaLS clean.

**2. Flow ergonomics + parametrization — RESOLVED (2026-07-15).** `test_each` and `describe` shipped;
   parametrized fixtures (`ctx:param`) and `f:use` were both assessed and **dropped** as magic that
   fights prova's explicit, lazy-`ctx:use` model (details in the sub-bullets and
   `docs/plans/phase1-ergonomics.md`). The parametrization story is now complete and explicit:
   `test_each` (data-driven tests) · separate suites (divergent variants) · profiles (env) ·
   `t:use`-in-steps (flow fixtures). No engine work remains here; the aspirational examples need only
   a live service backend (Phase 2), not a new authoring feature.
   - **`prova.test_each(name_tmpl, cases, fn)` — DONE.** One test per case; `{placeholder}`s in the
     name filled from the case; the case reaches the body as its 2nd arg *and* as `t.case`; returns
     the list of generated handles. Top-level + `GroupBuilder:test_each`. Implemented by threading an
     optional `case: Value` through `Node → PlanItem → Ctx` (no per-test Lua wrapper): `run_one`
     passes `(ctx, case)` so `fn(t, case)` and plain `fn(t)` (ignoring the trailing nil) both work,
     and `Ctx` exposes `t.case` via a field getter. Name templating = `render_case_name` (unknown key
     or non-table case leaves the `{key}` literal). *(`testdata/test_each.lua` + `tests/test_each.rs`:
     8 tests green, names substituted, `t.case`==arg, plain test unaffected.)*
   - **`ctx:param()` + `{ params = {...} }` on `prova.fixture`** — **DROPPED (2026-07-15).**
     Parametrized fixtures are pytest's most-confusing feature: a fixture silently *multiplies* the
     tests that transitively use it (action-at-a-distance). Prova's **lazy `ctx:use`** model can't even
     do the usage-driven version cleanly (no static fixture-dependency graph) — the architecture is
     telling us not to. And the real need decomposes without it, by whether the *assertions* are shared:
     same assertions over varying data → **`test_each`** (have it); divergent variants (relational CRUD
     vs document store) → **separate suites**; env-level → **profiles**. A `describe_each` (a `test_each`
     lifted to a whole block) is the only gap and is **not built speculatively** — add it if a real
     "whole block ×N, shared assertions" need appears. See `docs/plans/phase1-ergonomics.md`.
   - **`prova.describe(label, fn)` — DONE.** Ambient labeling group: bare `prova.test`/`test_each`/
     `group`/`flow` inside `fn` nest under `label`. Implemented via a **`parent_stack`** in the
     `Collector` (dynamic scoping): top-level declarations register under `current_parent()`;
     `prova.describe` pushes its labeling group, runs the body, pops (even on error). Structurally a
     group (labeling only, no new fixture scope). `GroupBuilder:describe` is the builder form (== a
     nested group). *(`testdata/describe.lua` + `tests/describe.rs`: 5 tests, nested labels in paths,
     pop-back-to-root verified.)*
   - **`f:use(fixture)`** on the FlowBuilder — **DROPPED (2026-07-15).** The flow builder runs at
     *collection*; a fixture *value* only exists at *execution*, so `f:use` could only work via magic
     (a transparent proxy that lies to `type()`, or re-running the flow builder). Re-running the
     builder is real infrastructure whose only substantial consumer is a **load executor** — and load
     testing is an explicit **non-goal** (`foundations.md`: "stays with k6/Gatling… measure timing,
     not model load"). So there's no principled consumer to justify it. Flow-scoped fixtures use
     `t:use` inside steps — scope-cached, so it's the same instance across steps — consistent with how
     fixtures resolve everywhere. `ordering.lua`/`dependent_flows.lua` rewritten to `t:use`.
   - **`describe_each` (a `test_each` lifted to a whole block)** — **not built; trigger documented.**
     Add it *only* when you catch: the same case-list copied across several `test_each` in one file,
     or copy-pasting a whole suite to change one variant constant, or wanting to run an existing
     `describe` block over N shared-assertion variants. It composes `describe` + `test_each` (both
     built), so it's a cheap additive add when a real need appears — not before.
   - *Graduation status:* **`rust_cli.lua` graduated** → `examples/rust_cli_test.lua` (needed only
     `describe`). It renders a **local, dependency-free Lua archetype** (`examples/fixtures/rust-cli`)
     rather than the remote `archetype-rust-cli` — remote archetypes are Rhai/v2 and prova-archetect
     is Lua/v3-only, so a local Lua archetype is the self-contained path — asserts the layout under
     `describe` with soft assertions, and `cargo build`s the output **offline** (`requires`-gated on
     cargo). *(`crates/prova-archetect/tests/rust_cli.rs`.)* Remaining aspirational files are no
     longer blocked on any authoring feature (`ctx:param` and `f:use` both dropped). `ordering`,
     `dependent_flows`, and `http_service` now need only a **live service backend** to graduate —
     tracked with the capstone (Phase 2), where a real (or `shell.spawn`ed) service exists to run them
     against.

### Phase 2 — Compose the North Star (the capstone)

**3. Ephemeral-infra recipes** — Postgres/MySQL/Pulsar as reusable fixtures. Mostly composition of
   existing `docker` + `db`, but capture the readiness patterns as helpers:
   - Postgres/MySQL: **connect-retry readiness** (they restart once at init — `pg_isready`/port are
     false-positives; retry `pcall(db.connect, url)` until it holds — see `examples/db_postgres_test.lua`).
   - **Pulsar:** `docker.run{ image = "apachepulsar/pulsar", ... }` running `bin/pulsar standalone`;
     readiness via `wait.log` (e.g. "messaging service is ready") or HTTP admin `:8080/admin/v2/...`.
     Producing/consuming: either (a) a small **`pulsar` module** (Rust `pulsar` crate) with
     `producer:send`/`consumer:receive`, or (b) drive it through the apps under test and assert via
     DB/HTTP. Prefer (b) for the first assembly; add a `pulsar` module if direct assertion is needed.
   - Consider a `net.free_port()` helper for locally-`shell.spawn`ed apps that need a dynamic port
     (containers already get random host ports from `docker.run`).

**4. The service archetypes** — confirm `archetect.render` works on the real service archetypes
   (`archetype-rust-service-tonic-workspace`, a Go REST equivalent). These pull over git + build.
   If archetypes are missing/awkward, **generate scaffolding on the fly** (per the dogfooding ethos:
   scaffold from the starters, fix the starter if lacking). Renders are in-process (fast); the
   `cargo build`/`go build` of the output is the slow part — make it a `suite`-scoped fixture.

**5. The full cross-service acceptance suite** — the capstone integration test: render both apps →
   provision postgres+mysql+pulsar → boot both wired to ephemeral endpoints (env from
   `docker` host-ports) → drive gRPC (Rust) + REST (Go) → query both DBs to assert cross-service
   state (e.g. an order placed via gRPC lands in Postgres, flows through Pulsar, and appears in
   MySQL via the Go consumer). Gate on `requires{docker,...}`. This is the proof of the whole thesis.

   *Note (2026-07-16):* `mock` does **not** discharge this item and must not be mistaken for it. The
   capstone's whole point is that both services are **real** — an order really crossing gRPC →
   Postgres → Pulsar → MySQL. Mocking either service away would make it pass while proving nothing
   (`proof-driven-development.md:87-89`). Where `mock` *does* help is at the suite's edges (a
   third-party dependency neither service owns) and in asserting the interactions *between* the real
   services once `mock`'s passthrough gains the network vantage — see `docs/plans/mocks.md` §C2/C3.

### Phase 3 — Scale & polish (daemon-independent, any order)

6. **`graphql` module** (same async-module shape as `http`/`grpc`).
7. **Snapshots — Phase A DONE (core); B/C in progress.** `t:expect(str):matches_snapshot(name?)`
   compares against a `.snap` colocated with the test (`<dir>/snapshots/<stem>__<key>.snap`);
   `--update-snapshots`/`-u` (re)writes. Missing → fail + reviewable `.snap.new`; mismatch → fail with
   an LCS line diff; robust `---`-delimited doc format. Plumbing: `Collector.file_paths` →
   `RunState` → `TestRun.snapshot`; `RunConfig.update_snapshots`. (engine.rs; unit tests + a dogfooding
   self-test.) **Phase B DONE:** the `level` dial — a string subject snapshots as content; any **path
   handle** (a table with a `path` field: `archetect.render` output, `out:file(rel)`) snapshots at
   `level = "layout"` (sorted relative paths — the render shape, default for a directory) or `"content"`
   (paths + bytes, `=== path ===` sections). Anti-rot default lives in the API (broad dir → cheap shape;
   opt into content). Verified: a layout snapshot catches an added file with a `+ path` diff.
   **Phase C DONE:** `--unreferenced ignore|warn|delete` flags/removes `.snap` files no test
   referenced (`warn` fails the run so CI catches rot). Opt-in and **full-run-only** — skipped with a
   note under any selection — and it only scans dirs where a snapshot *was* referenced, so a
   deselected file's snapshots are never examined (no false positives). Shared cross-worker registry
   (`SnapshotRegistry`) + `unreferenced_snapshots`. **Snapshots complete** (A+B+C). Design:
   `docs/plans/snapshots.md`.
8. **Selectors** — tag expressions (`--tags`), `-k` name filter, `--last-failed`, sharding.
9. **Reporters — DONE, then productionized.** JUnit XML (`--junit PATH`, a *file* sink that composes
   with any `--format`, the CI lingua franca) and TAP (`--format tap`, a stdout stream) both landed as
   `Reporter`s over the event seam. JUnit buffers cases and writes one `<testsuites>` doc on
   `RunFinished` — path split into `classname`/`name`, XML-escaped, failure/skipped elements, valid XML
   (xmllint-checked). TAP streams `TAP version 13` + `ok/not ok N` (with `# SKIP` directives and YAML
   failure diagnostics) + a trailing `1..N` plan. *(`model.rs` `JUnitReporter`/`TapReporter`; verified
   live via the CLI.)*
   The output overhaul then landed on top of the same seam: **source locations** (file + line captured
   from the Lua stack at declaration) travel `Node → PlanItem → NodeResult → Event::NodeFinished` into
   every sink (JSONL `file`/`line` keys, JUnit `file=`/`line=` attrs, TAP diagnostics, MCP failures);
   the CLI's **HumanReporter** (`prova-cli/src/report.rs` — core stays unstyled) adds color
   (anstream/anstyle: auto TTY detection, `NO_COLOR`, `--color`, manifest `color`), skip reasons
   inline, a `failures:` recap with `--node` rerun lines, and `-q/--quiet`; a **GitHubReporter**
   auto-detects `GITHUB_ACTIONS` (`--gha`, `PROVA_GHA`, manifest `github`) and emits
   `::error file=,line=` PR annotations + a `$GITHUB_STEP_SUMMARY` markdown table; JUnit gained
   `timestamp`/`errors`/`assertions` attrs, `<properties>`, the package name as suite name, and a
   manifest `junit = "path"` key. Config precedence everywhere: CLI flag > env > manifest > auto.
   *Deliberate non-features:* no pattern-string format DSL (curated formats; `--format json` + the
   `Reporter` trait are the escape hatches) and no per-suite `::group::` folding (parallel suites
   interleave through one coordinator channel). *Follow-up:* a `FailureKind` on the event to split
   JUnit `<error>` from `<failure>`.
10. **Load executor — NON-GOAL (not on the roadmap).** The clean definition≠execution split means an
    alternate load driver *could* be dropped in over the same plan — but load/performance testing is
    an explicit non-goal (`foundations.md`: "stays with k6/Gatling… measure timing, not model load").
    We keep the architectural door open as a property of good layering, not as a feature to ship.
    Consequently the **re-runnable-flow-*builder*** extension it would need is **not** planned (test
    and step bodies are already re-runnable via `run_one`; only the structural flow *builder* runs
    once, like any declarator — correctly). This is why `f:use` was dropped, not built.
11. **bollard depth** — health checks, log *follow* (streaming), typed inspect for richer waits.
12. **Cross-worker `suite` fixtures** — the one open semantic: a serialized once-guard for
    serializable values across per-worker Lua states (`suite.rs` note).

## Key context a fresh session must not rediscover

(Full detail is in the `prova-test-framework` auto-memory; the load-bearing ones:)

- **`!Send` shapes everything.** `mlua::Lua` + collected `Function` bodies are `!Send`. The parallelism
  boundary is the **file** (per-worker Lua state). Anything crossing threads must be `Send` (owned).
- **Async boundary discipline.** Extract owned values off the Lua boundary *before* an `await`; never
  hold a Lua borrow or `RefCell` guard across `.await`. Async mlua methods: `add_async_method(_mut)`,
  `Fn(Lua, UserDataRef(Mut)<T>, A) -> Future + 'static` — clone cheap `Rc`/handles into the future.
- **`db.connect` / `docker.run` / any async module call must run in a fixture or test body**, never at
  file top level (collection runs synchronously, outside the runtime).
- **DB placeholders are backend-native** (`$1` Postgres, `?` MySQL/SQLite). sqlx `Any` reports
  **no type kind for computed columns** (SQLite `count(*)`) → `extract_untyped` probe fallback.
- **Container/DB readiness = retry the real thing**, not `pg_isready`/port-open (init restarts).
- **Docker `:exec` needs a shell in the image** (`sh -c`); `traefik/whoami` is `FROM scratch`.
- Feature flags: `http`, `db`, `docker` are default-on; the crate builds with `--no-default-features`.
- **Verify every change:** `cargo test` (39 tests, some Docker/cargo-gated), `cargo clippy --all-targets`
  (zero warnings), `lua-language-server --check "$(pwd)"` (LuaLS-clean), and run touched
  `examples/*.lua` via the CLI. Keep the LuaCATS stub (`library/`) in lockstep with the runtime.

## Resume checklist

1. Read auto-memory `prova-test-framework`; skim this file + `architecture.md` "Current status".
2. `cd /Users/jimmie/personal/archetect/prova && cargo test && cargo clippy --all-targets` — green baseline.
3. Confirm Docker: `docker info`. If up, the `docker`/`db_postgres` tests run for real.
4. Pick the next increment (Phase 1 → `grpc`). Make the invocation-strategy decision, build, verify,
   commit with the established message style, update memory + `architecture.md`.
