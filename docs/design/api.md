# Assay — Test & Fixture API Design

> Status: **design draft** — we are nailing the authoring surface before building the
> Rust engine. Nothing here is implemented yet. The goal of this document is to make the
> Lua DSL feel right in example code first, then work backward to the runtime.

## What Assay is

A **programmable, language-agnostic acceptance-test runner**: a real scripting language
(Lua) plus a real fixture model, shipped as a single static binary. It is not a unit-test
framework (JUnit/pytest own that inside their languages) and it is not a single-protocol
tool (Hurl owns HTTP). It occupies the **black-box acceptance/integration layer**: bring a
system into existence (render it, build it, boot it), then poke it with shell + HTTP +
filesystem assertions, with fixtures holding the setup/teardown together.

The wedge over the existing agnostic testers (Hurl, Bats, Venom, Robot Framework, goss) is
deliberate:

- They are **single-domain** or **declarative YAML/Gherkin**. The moment a test needs a
  loop, a computed value, a conditional, or reusable setup, YAML hits a wall.
- None of them have a **fixture model** — scoped setup/teardown with dependency injection
  and caching. That is the thing pytest users cannot live without, and it is structurally
  impossible in a declarative format.

So the two things this document must get right are the **fixture model** and the
**assertion surface**. Everything else (discovery, reporting, the http/shell modules) is
comparatively mechanical.

## Architectural stance (informs the API)

The core runner is **domain-agnostic**. Archetype rendering is a *plugin*, not a built-in.

```
assay-core      → discovery, fixtures, assertions, reporting, the `assay`/`ctx`/`expect` surface
  modules (first-party plugins):
    fs          → file/dir handles, exists/contains/snapshot
    shell       → run commands, assert exit/stdout/stderr
    http        → blocking get/post, assert status/body/json
    archetect   → render(source, answers) in-process via archetect-core   ← the justifying use case
```

`assay-core` has **zero** archetect-specific assumptions. The `archetect` module is the
first-party plugin that justifies the whole runner's existence and is our dogfooding
target, but it sits behind the same plugin boundary that a `docker` or `terraform` module
would. This lets the general-purpose framework *emerge* from a real shipped use case
instead of being speculative.

---

## The fixture model

This is the heart of the design. Borrowed conceptually from pytest, adapted to honest Lua
idioms (no decorators, no parameter-name reflection).

### Declaring a fixture

```lua
local assay = require("assay")

-- assay.fixture(name, scope, factory)
-- scope: "test" (default) | "file" | "suite"
assay.fixture("workspace", "file", function(ctx)
  local dir = fs.tempdir()
  ctx:defer(function() fs.remove_all(dir) end)   -- teardown, LIFO
  return dir                                       -- the fixture value
end)
```

A fixture is a **named factory** that produces a value. It may register teardown via
`ctx:defer(fn)` (Go-style; runs in LIFO order when the scope ends) and may depend on other
fixtures via `ctx:use(name)`.

We chose `ctx:defer()` over a coroutine `yield`-style teardown (pytest's `yield` fixtures)
because it is trivial to drive across the Rust/Lua boundary, supports multiple cleanups
per fixture, and reads clearly. A `yield`-style sugar could be layered on later, but there
is exactly **one** blessed way for v1.

### Requesting fixtures (dependency injection)

Lua cannot reliably reflect a function's parameter names, so we do **not** auto-inject by
signature the way pytest does. Injection is **explicit and lazy** via `ctx:use(...)`.

`assay.fixture` **returns a handle**; you pass that handle to `use`:

```lua
local workspace = assay.fixture("workspace", "file", function(ctx)
  return ctx:tempdir()                 -- factory returns string
end)                                   -- workspace : assay.Fixture<string>

assay.test("renders into a clean workspace", function(t)
  local ws = t:use(workspace)          -- ws : string — type flows through (see LSP note)
  ...
end)
```

Handle-based (not string-based) `use` is a **deliberate, LSP-driven decision**. With
LuaCATS generics, `assay.fixture(...)` returns `assay.Fixture<T>` (T = the factory's return
type) and `ctx:use(handle)` recovers `T` at the call site — full completion and
type-checking on fixture values. A string key (`use("workspace")`) is a type black hole:
LuaLS can only type it as `any`. A bare-string overload is retained as an **escape hatch**
for cross-file lookup by name, but it is untyped; prefer the handle.

Explicit `use` (either form) also buys three things pytest's parameter-name magic hides:
1. **Laziness** — a fixture is only built when something actually asks for it.
2. **Traceability** — you can grep for the handle/name to find every dependent.
3. **No name-collision surprises** between fixtures and local variables.

Fixtures depend on other fixtures the same way — capture the handle, `use` it:

```lua
local rendered_project = assay.fixture("rendered_project", "file", function(ctx)
  local ws = ctx:use(workspace)              -- fixture-to-fixture dependency (typed)
  return archetect.render{ source = "…", destination = ws, defaults = true }
end)
```

Cross-file fixtures: a `conftest`-equivalent module (`assay.lua`) can `return` a table of
handles that sibling test files `require`, keeping types intact without a global registry.

### Scopes and caching

| Scope    | Instantiated              | Torn down                | Cache key                     |
|----------|---------------------------|--------------------------|-------------------------------|
| `test`   | first `use` in a test     | after that test          | (fixture, test invocation)    |
| `flow`   | first `use` in a flow     | after the flow's steps   | (fixture, flow invocation)    |
| `file`   | first `use` in a file     | after all tests in file  | (fixture, file)               |
| `suite`  | first `use` in the run    | after the whole run      | (fixture, run)                |

The `flow` scope is only valid inside a `flow` (see [Execution model](#flows--ordered-steps-with-shared-context));
declaring it elsewhere is a collection error. Inside a flow, `test` scope means **per-step**.

Caching is **per scope instance**: two tests in the same file that both `use("rendered_project")`
(scope `file`) share one render; its teardown runs once, after the last test in the file.
A `test`-scoped fixture is rebuilt fresh for every test.

Teardown order is **LIFO within a scope**, and scopes tear down inner-to-outer
(`test` before `file` before `suite`). A fixture's `defer`red cleanups run before the
cleanups of any fixture it depended on — dependencies outlive their dependents.

### Autouse fixtures

A fixture can opt into running even when no test names it — useful for ambient setup
(seeding a temp HOME, starting a mock server):

```lua
assay.fixture("mock_registry", { scope = "suite", autouse = true }, function(ctx)
  local server = http.serve_mock({ ["/v1/index"] = { status = 200, json = {...} } })
  ctx:defer(function() server:stop() end)
  return server
end)
```

(The options-table form `{ scope=..., autouse=... }` is accepted anywhere the bare scope
string is; the string is just sugar for `{ scope = "..." }`.)

### Parametrized fixtures

A fixture can be **parametrized**, producing one variant per parameter. Every test that
uses it is multiplied across the variants (this is pytest's most powerful and most-copied
feature):

```lua
local toolchain = assay.fixture("toolchain", "suite", function(ctx)
  local tc = ctx:param()               -- "stable" or "nightly"
  return { name = tc, cargo = "cargo +" .. tc }
end, { params = { "stable", "nightly" } })

assay.test("builds on the toolchain", function(t)
  local tc = t:use(toolchain)
  local r = shell.run(tc.cargo .. " build", { cwd = t:use(workspace) })
  t.expect(r.code):equals(0)
end)
-- Runs twice: "builds on the toolchain[stable]" and "…[nightly]".
```

---

## Tests

```lua
-- assay.test(name, factory)   or   assay.test(name, opts, factory)
assay.test("builds cleanly", function(t)
  ...
end)

assay.test("flaky network path", { retries = 2, timeout = "30s", tags = { "net" } }, function(t)
  ...
end)
```

### Parametrized tests (table-driven)

```lua
-- assay.test_each(name_template, cases, factory)
assay.test_each("renders for {lang}", {
  { lang = "rust", entry = "src/main.rs" },
  { lang = "java", entry = "src/main/java/App.java" },
}, function(t, case)
  local out = archetect.render{ source = "…", answers = { language = case.lang }, defaults = true }
  t.expect(out:file(case.entry)):exists()
end)
```

`{lang}` in the name template is filled from the case table so each row reports as a
distinct test.

### Grouping

`describe` is **organizational labeling** in v1 (nested names in the report), not a new
fixture scope. `before_each`/`after_each`/`before_all`/`after_all` attach to the enclosing
file (or `describe` block) and are convenience wrappers over autouse fixtures:

```lua
assay.describe("rust cli archetype", function()
  assay.before_all(function() ... end)

  assay.test("has a Cargo.toml", function(t) ... end)
  assay.test("compiles", function(t) ... end)
end)
```

> Open question: whether `describe` should introduce a real nested fixture scope (jest
> semantics). Deferred — labeling-only keeps the scope model to three clean levels for v1.

---

## Execution model: ordering, isolation & concurrency

Ordering and isolation are stated up front, because being surprised by them is the fastest
way a suite loses trust. The rules are deliberately boring and explicit.

**The four rules**

1. **Definition order is the default, and it is deterministic.** Tests run top-to-bottom in
   declared order within a file; files run in a stable, sorted order. No randomization, no
   reordering behind your back — what you read is what runs.
2. **Isolation is the default *state* model.** Tests never share mutable state unless you
   deliberately create it (a broader-scoped fixture, or a flow). Enforced by the *shape of
   the API*, not by OS sandboxing — see below.
3. **Ordering, when you need it, is a first-class construct — never a trick.** Reach for a
   `flow` (linear, shared context) or `depends_on` (a DAG edge). Both are one line and their
   semantics — including *skip-downstream-on-failure* — are guaranteed.
4. **Parallelism is opt-in and never violates rules 1–3.** The runner is serial by default
   (safest for side-effecting acceptance tests). `--jobs N` turns on concurrency; the
   resource scheduler then runs independent tests in parallel while honoring every declared
   order and dependency.

The key move is separating two knobs most frameworks conflate: **isolation** (do tests share
state?) and **order** (what sequence do they run in?). Go couples them — it randomizes order
*to force* isolation. We don't. With isolation as the default state model, definition order
is safe *and* predictable: you can't accidentally depend on order because there is no ambient
state to leak, and when you *want* to depend on order, you say so.

**Why not randomize by default?** Randomized order (Go, pytest-randomly) exists to catch
accidental coupling in tests that are *supposed* to be isolated — a unit-testing concern. In
acceptance/integration testing, ordered-with-shared-context is a legitimate and often
*dominant* shape, so randomizing by default fights the domain and surprises authors: the
"purity nobody asked for." We get the same safety another way (isolation by construction) and
offer randomization as an **opt-in hardening pass**:

```
assay test --shuffle           # random order, prints a seed
assay test --shuffle=1234      # replay that exact seed
```

**What you give up — and what going further would cost.** Choosing definition-order +
opt-in-shuffle over randomize-by-default costs you *automatic* coupling detection on every
run; isolation-by-construction buys most of it back, and `--shuffle` recovers the rest on
demand. Going the *other* direction — a fully-ordered suite where every test implicitly
continues the previous one — is what we explicitly reject: it forfeits parallelism, destroys
fault isolation (one failure makes everything after it meaningless), makes a single test
un-runnable alone, and turns inserting a test into a landmine. Explicit `flow`/`depends_on`
give real ordering exactly where you declare it, while everything else stays independent and
parallelizable. **Ordering you declare is a guarantee; isolation you don't override is a
guarantee. Both are easy; neither is a surprise.**

### Isolation — enforced by API shape, not sandboxing

"Hermetic by default" is an honest claim here because the DSL gives you **no ambient mutable
global state to leak**:

- There is no implicit working directory and no `os.chdir`. Every side-effecting call takes
  its context explicitly: `shell.run(cmd, { cwd = …, env = … })`, `fs.read(path)`.
- Scratch space is `ctx:tempdir()` — unique per scope, auto-removed.
- The only ways to share state across tests are the *tracked, scoped* ones: a broader-scoped
  fixture, or a flow's context. Both appear in the report and tear down deterministically.

Two tests therefore cannot corrupt each other through the framework's own surface. That is
what lets definition order be the safe default without a randomizer policing it.

### Flows — ordered steps with shared context

A `flow` is the go-to when you *do* need order plus built-up state — the create → read →
update → delete shape that dominates integration testing. Steps run in declared order, share
the enclosing scope, and **once a step fails the remaining steps are skipped** (reported as
skipped-due-to-upstream; the flow is marked failed):

```lua
local api = "http://localhost:8080"

assay.flow("order lifecycle", function(flow)
  local order                       -- shared by all steps (plain Lua closure)

  flow:step("create", function(t)
    order = http.post(api .. "/orders", { json = { sku = "widget", qty = 2 } }):json()
    t.expect(order.id):is_truthy()
  end)

  flow:step("read back", function(t)          -- skipped if "create" failed
    local res = http.get(api .. "/orders/" .. order.id)
    t.expect(res.status):equals(200)
    t.expect(res:json().qty):equals(2)
  end)

  flow:step("delete", function(t)
    t.expect(http.post(api .. "/orders/" .. order.id .. "/cancel").status):equals(204)
  end)
end)
```

- A flow is **one scheduling unit**: its steps always run serially, in order, on one worker —
  never split or parallelized. Independent flows/tests still parallelize around it.
- Flow-level fixtures via `flow:use(fixture)` live for the flow's lifetime (`flow` scope).
  Inside a flow, `test`-scoped fixtures are per-**step**.
- Flow-level opts (`tags`, `resources`, `depends_on`) apply to the whole flow.
- A step may `t:skip(reason)` itself; a step is the flow's analog of a test for reporting.

Flows are the sanctioned home for shared mutable state — reach for them instead of widening a
fixture's scope just to smuggle state between tests.

### Dependencies — `depends_on`

When the relationship is a graph rather than a tidy line (a test needs another's *success*,
but they aren't a single sequence), declare an edge. `assay.test` and `assay.flow` return
**handles** (like fixtures); pass them to `depends_on`:

```lua
local seeded = assay.test("seed reference data", function(t) ... end)

assay.test("report reflects seed", { depends_on = { seeded } }, function(t) ... end)
-- If `seeded` fails or is skipped, this test is SKIPPED (not failed), with the reason.
```

- The runner topologically sorts; a cycle is a collection-time error.
- Independent subgraphs run in parallel under `--jobs`; dependency edges are always honored.
- **`depends_on` gates on pass/fail; it does not transfer state.** If a dependent also needs
  the upstream's *data*, share it through a fixture (or use a flow for a linear sequence).
  Keeping "did it pass?" separate from "give me its value" is deliberate — conflating them is
  exactly how implicit, brittle ordering creeps in.

### Resources & parallelism

Under `--jobs N`, tests run concurrently. Shared *external* resources — a port, a database,
an account, a file path — make that unsafe unless declared. A test or flow lists the
resources it needs; the scheduler guarantees safe co-scheduling:

```lua
assay.test("boots on :8080",   { resources = { "port:8080" } },       function(t) ... end)  -- exclusive
assay.test("also wants :8080", { resources = { "port:8080" } },       function(t) ... end)  -- never concurrent with the above
assay.test("reads shared db",  { resources = { assay.shared("db") } },function(t) ... end)  -- concurrent with other shared("db")
```

- A bare string token is **exclusive**: no two holders run at once. `assay.shared(token)` is a
  concurrent reader — readers run together, but an exclusive holder waits for all readers to
  release (a readers-writer lock over the token namespace).
- `{ serial = true }` is sugar for a single process-wide exclusive resource — the escape hatch
  for "never run this alongside anything."
- Resource declarations are inert under the serial default and fully enforced under `--jobs`,
  so you can declare them up front and scale out later without touching the tests.

---

## The test/fixture context (`t` / `ctx`)

Tests receive a **TestContext** (`t`); fixtures receive a **Context** (`ctx`). They share a
base; the test context adds assertions and skip/case.

| Member            | On       | Purpose                                                        |
|-------------------|----------|---------------------------------------------------------------|
| `ctx:use(name)`   | both     | Instantiate/fetch a fixture value (lazy, scope-cached)        |
| `ctx:defer(fn)`   | both     | Register LIFO teardown for the current scope                  |
| `ctx:tempdir()`   | both     | A scratch dir auto-removed when the scope ends                |
| `ctx:log(msg)`    | both     | Structured log line attached to the test/fixture in the report|
| `ctx:param()`     | fixtures | Current parameter (parametrized fixtures)                     |
| `t.expect(v)`     | tests    | Start a fluent assertion (see below)                          |
| `t:skip(reason)`  | tests    | Skip the current test at runtime                              |
| `t.name`          | tests    | The resolved test name                                        |
| `t.case`          | tests    | The current case table (parametrized tests)                   |

`t.expect` is a bound callable field (matches `t.expect(out:file("x"))`); `use`/`defer`/
`tempdir` are methods (colon).

---

## Assertions

A single fluent entry point, `expect(subject)`, returning a matcher. Matchers validate the
subject's type at call time, so domain subjects (file handles, shell results) get rich
checks.

```lua
t.expect(r.code):equals(0)
t.expect(r.stdout):contains("Compiling")
t.expect(r.stdout):matches("Finished .+ in %d")     -- Lua pattern
t.expect(out:file("Cargo.toml")):exists()
t.expect(out:dir("target")):never():exists()         -- negation
t.expect(res.status):is_one_of({ 200, 204 })
t.expect(value):is_nil()
t.expect(list):has_length(3)
```

### Core matchers (v1)

| Matcher                     | Passes when …                                         |
|-----------------------------|-------------------------------------------------------|
| `:equals(x)` / `:eq(x)`     | deep-equal to `x`                                     |
| `:is_truthy()` / `:is_falsy()` | Lua truthiness                                     |
| `:is_true()` / `:is_false()`| strictly boolean                                      |
| `:is_nil()`                 | value is nil                                          |
| `:contains(x)`              | substring (strings) or membership (tables)            |
| `:matches(pat)`             | Lua-pattern match (strings)                           |
| `:has_length(n)`            | `#subject == n`                                       |
| `:is_one_of(t)`             | membership in `t`                                     |
| `:gt(n)` `:gte(n)` `:lt(n)` `:lte(n)` | numeric comparison                          |
| `:exists()` / `:is_file()` / `:is_dir()` / `:is_empty()` | filesystem handle checks   |

`:never()` returns a negated matcher (`t.expect(x):never():contains("secret")`).

### Soft assertions

By default a failed assertion **fails the test immediately**. `t.expect_all(function() … end)`
collects multiple failures before failing (useful for asserting many files at once):

```lua
t.expect_all(function()
  t.expect(out:file("README.md")):exists()
  t.expect(out:file("LICENSE")):exists()
  t.expect(out:file(".gitignore")):exists()
end)  -- reports every missing file, not just the first
```

### Snapshots

We already lean on Rust's `insta` in the archetect workspace; the file module exposes the
same idea to Lua for whole-file / whole-tree snapshotting:

```lua
t.expect(out:file("src/main.rs")):matches_snapshot()      -- .snap alongside the test file
t.expect(out:tree()):matches_snapshot("full-layout")      -- named snapshot of the rendered tree
```

`assay test --update-snapshots` rewrites them, mirroring `cargo insta`.

---

## Modules (first-party plugins)

All are `require`-able and globally available inside test files.

### `fs`

```lua
fs.tempdir()               -- create a temp dir (not auto-cleaned; pair with ctx:defer or ctx:tempdir)
fs.remove_all(path)
fs.read(path)              -- string
fs.exists(path)            -- bool
fs.glob(root, "**/*.rs")   -- list of paths
```

Rendered output and `fs` both yield **tree/file handles**:

```lua
local out = archetect.render{ … }          -- out is a tree handle rooted at the destination
out.path                                    -- absolute root
out:file("src/main.rs")                     -- file handle (relative to root)
out:dir("src")                              -- dir handle
out:file("x"):read()                        -- string contents
out:tree()                                  -- serializable snapshot of the whole layout
```

### `shell`

```lua
local r = shell.run("cargo build", {
  cwd = out.path,
  env = { RUST_LOG = "info" },
  timeout = "120s",
  check = false,            -- if true, non-zero exit raises instead of returning
})
r.code        -- integer exit code
r.stdout      -- string
r.stderr      -- string
r.duration    -- seconds (number)
r:ok()        -- r.code == 0
```

### `http` (blocking in v1)

Deliberately synchronous. Test suites are IO-bound but rarely need in-test concurrency;
parallelism lives at the test-case level in the Rust runner, not inside a test.

```lua
local res = http.get("http://localhost:8080/health", { headers = { Accept = "application/json" } })
res.status                 -- integer
res.body                   -- string
res.headers                -- table
res:json()                 -- decoded table (raises on non-JSON)

http.post(url, { json = { name = "widget" }, timeout = "10s" })

-- retry helper for boot-then-probe flows
http.wait_for("http://localhost:8080/health", { status = 200, timeout = "30s", every = "500ms" })
```

### `archetect` (the justifying plugin)

Renders **in-process** via `archetect-core` — the single biggest advantage over a
pytest-style subprocess harness. Prompt answers are passed as data (no `-a k=v` string
marshaling), errors surface as real diagnostics, and we can assert on the IO-protocol
write operations, not just the post-hoc filesystem.

```lua
local out = archetect.render{
  source = "https://github.com/archetect/archetype-rust-cli.git",  -- or a local path
  answers = { project_name = "widget", description = "demo" },
  switches = { "ci" },
  defaults = true,                 -- use defaults for anything unanswered (headless)
  destination = t:tempdir(),       -- optional; a temp dir is used if omitted
}

out:file("Cargo.toml")             -- tree handle, as above
out.writes                         -- ordered list of IO-protocol write ops the render intended
```

---

## Discovery, layout, CLI

- Test files match `**/*_test.lua` (and `**/*.test.lua`). Each file is one `file` scope.
- A directory tree of test files is a **suite**; `suite`-scoped fixtures span the run.
- Fixtures/helpers can live in a `conftest.lua`-equivalent (`assay.lua`) loaded before the
  test files in its directory and inherited by subdirectories.

```
$ assay test                     # discover & run under CWD
$ assay test tests/rust_cli      # a subtree
$ assay test -k "compiles"       # filter by name substring
$ assay test --tags net          # filter by tag
$ assay test --update-snapshots
$ assay test --jobs 8            # test-case-level parallelism (fixtures respect scope)
$ assay test --format tap|pretty|json
```

Two front doors over the same `assay-core` lib (mirrors `archetect-core` ← `archetect-bin`):
1. `assay` — the standalone binary (general-purpose positioning).
2. `archetect test` — the same runner surfaced as an archetect subcommand for archetype
   authors who already have the CLI. *"The generator ships its own test framework."*

---

## Tooling / LSP (decided up front)

Authoring quality is a first-class goal, so the LSP story is settled before the engine:

- **LuaCATS via `lua-language-server`.** LuaCATS is the annotation dialect LuaLS consumes
  (`---@class`, `---@param`, generics, `---@meta`). It gives types through comments with
  **zero build step** — the right fit for a DSL where authors write plain Lua. We reject
  Teal (a compile-to-Lua typed dialect) because it would force authors to write `.tl` and
  compile. This also matches archetect's existing choice for its own Lua API annotations.
- **Target Lua 5.4**, matching archetect's mlua (`features = ["lua54"]`). `runtime.version`
  in `.luarc.json` is pinned to `Lua 5.4`.
- **Annotations are the authoritative surface.** `library/assay.lua` + `library/modules.lua`
  (`---@meta` stubs) define the API; the runtime must conform to them. Risk to manage:
  drift between hand-written stubs and the Rust-registered API. For the POC we hand-maintain;
  longer term we may generate the stubs from the Rust registration to guarantee they match.
- **Generics carry fixture types.** `assay.fixture` → `assay.Fixture<T>`; `ctx:use(handle)`
  → `T`. This is *why* `use` takes a handle, not a string (see DI section).
- **Distribution mirrors `archetect ide setup`.** An `assay ide setup` command will
  `include_str!` the stubs, install them to the data dir, and write/update a `.luarc.json`
  (`runtime.version` + `workspace.library`). The repo checks in a `.luarc.json` pointing at
  `library/` so the examples are typed during prototyping right now.

## Open questions (to resolve while prototyping the engine)

1. **`describe` scoping** — labeling-only (current) vs. real nested fixture scope (jest).
2. **Fixture finalization on failure** — if a test fails mid-way, teardown still runs; do
   we surface teardown errors as separate failures or attach them to the test?
3. **Parallelism vs. `suite`/`file` fixtures** — a `file`-scoped fixture is naturally
   serialized within its file; across files we can parallelize freely. Confirm the cache
   is keyed so parallel workers don't double-instantiate a `suite` fixture (needs a
   once-guard in the runtime).
4. **`http` async** — keep blocking for v1; revisit only if a real suite needs in-test
   concurrency.
5. **Assertion library: build vs. vendor** — leaning build (~150 lines, our idioms) over
   vendoring busted, which assumes LuaRocks/standalone Lua and won't map onto embedded
   mlua cleanly. Same "vendored fork on our terms" stance as MiniJinja/inquire.
