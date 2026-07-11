# Prova — Test & Fixture API Design

> Status: **design draft** — we are nailing the authoring surface before building the
> Rust engine. Nothing here is implemented yet. The goal of this document is to make the
> Lua DSL feel right in example code first, then work backward to the runtime.

## What Prova is

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
prova-core      → discovery, fixtures, assertions, reporting, the `prova`/`ctx`/`expect` surface
  modules (first-party plugins):
    fs          → file/dir handles, exists/contains/snapshot
    shell       → run commands, assert exit/stdout/stderr
    http        → blocking get/post, assert status/body/json
    archetect   → render(source, answers) in-process via archetect-core   ← the justifying use case
```

`prova-core` has **zero** archetect-specific assumptions. The `archetect` module is the
first-party plugin that justifies the whole runner's existence and is our dogfooding
target, but it sits behind the same plugin boundary that a `docker` or `terraform` module
would. This lets the general-purpose framework *emerge* from a real shipped use case
instead of being speculative.

---

## The fixture model

This is the heart of the design. Borrowed conceptually from pytest, adapted to honest Lua
idioms (no decorators, no parameter-name reflection).

### Declaring a fixture

The runtime injects `prova` and the module globals (`fs`, `shell`, `http`, `archetect`) into
every test file — **no `require` is needed**. (`require("prova")` still works and returns the
same table, for anyone who prefers an explicit import.)

```lua
-- prova.fixture(name, scope, factory)
-- scope: "test" (default) | "file" | "suite"
prova.fixture("workspace", "file", function(ctx)
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

`prova.fixture` **returns a handle**; you pass that handle to `use`:

```lua
local workspace = prova.fixture("workspace", "file", function(ctx)
  return ctx:tempdir()                 -- factory returns string
end)                                   -- workspace : prova.Fixture<string>

prova.test("renders into a clean workspace", function(t)
  local ws = t:use(workspace)          -- ws : string — type flows through (see LSP note)
  ...
end)
```

Handle-based (not string-based) `use` is a **deliberate, LSP-driven decision**. With
LuaCATS generics, `prova.fixture(...)` returns `prova.Fixture<T>` (T = the factory's return
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
local rendered_project = prova.fixture("rendered_project", "file", function(ctx)
  local ws = ctx:use(workspace)              -- fixture-to-fixture dependency (typed)
  return archetect.render{ source = "…", destination = ws, defaults = true }
end)
```

Cross-file fixtures: a `conftest`-equivalent module (`prova.lua`) can `return` a table of
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
prova.fixture("mock_registry", { scope = "suite", autouse = true }, function(ctx)
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
local toolchain = prova.fixture("toolchain", "suite", function(ctx)
  local tc = ctx:param()               -- "stable" or "nightly"
  return { name = tc, cargo = "cargo +" .. tc }
end, { params = { "stable", "nightly" } })

prova.test("builds on the toolchain", function(t)
  local tc = t:use(toolchain)
  local r = shell.run(tc.cargo .. " build", { cwd = t:use(workspace) })
  t:expect(r.code):equals(0)
end)
-- Runs twice: "builds on the toolchain[stable]" and "…[nightly]".
```

---

## Tests

```lua
-- prova.test(name, factory)   or   prova.test(name, opts, factory)
prova.test("builds cleanly", function(t)
  ...
end)

prova.test("flaky network path", { retries = 2, timeout = "30s", tags = { "net" } }, function(t)
  ...
end)
```

### Parametrized tests (table-driven)

```lua
-- prova.test_each(name_template, cases, factory)
prova.test_each("renders for {lang}", {
  { lang = "rust", entry = "src/main.rs" },
  { lang = "java", entry = "src/main/java/App.java" },
}, function(t, case)
  local out = archetect.render{ source = "…", answers = { language = case.lang }, defaults = true }
  t:expect(out:file(case.entry)):exists()
end)
```

`{lang}` in the name template is filled from the case table so each row reports as a
distinct test.

### Grouping

`describe` is **organizational labeling** in v1 (nested names in the report), not a new
fixture scope. `before_each`/`after_each`/`before_all`/`after_all` attach to the enclosing
file (or `describe` block) and are convenience wrappers over autouse fixtures:

```lua
prova.describe("rust cli archetype", function()
  prova.before_all(function() ... end)

  prova.test("has a Cargo.toml", function(t) ... end)
  prova.test("compiles", function(t) ... end)
end)
```

> Open question: whether `describe` should introduce a real nested fixture scope (jest
> semantics). Deferred — labeling-only keeps the scope model to three clean levels for v1.

---

## Execution model: strategy is declared by the container

Every test lives inside a **strategy container** that determines how its children run. You do
not configure execution with CLI flags or a global default — you read the container and you
know. There are exactly two, each with a fixed, nameable guarantee, and each is designed so
that **invalid states are unrepresentable**: a container only exposes the capabilities its
strategy permits.

| Container     | Strategy    | Children                    | Order                        | Shared context             | Concurrency                 |
|---------------|-------------|-----------------------------|------------------------------|----------------------------|-----------------------------|
| `prova.group` | independent | tests, flows, nested groups | unspecified (don't rely)     | **none — not representable** | children parallelizable     |
| `prova.flow`  | sequence    | ordered steps               | declared order               | **the flow context**       | steps serial, one worker    |

- A **`group`** is a bag of independent units: isolated, unordered, parallelizable. Its
  `GroupBuilder` exposes `test`/`flow`/`group` — and **no shared-state mechanism**, so
  cross-child built-up context cannot be written. (Group children may also run on separate
  workers, so a smuggled closure variable wouldn't work anyway — the execution model enforces
  what the API already forbids.)
- A **`flow`** is an ordered sequence. Its `FlowBuilder` exposes `step` plus a shared scope;
  steps run in declared order on one worker and **cascade-skip** once a step fails.

This is *make-invalid-states-unrepresentable* applied to execution: shared mutable context is
a **flow-only capability**, granted by the `FlowBuilder`. A `group` never receives it, so
"ordered tests quietly sharing state" is not a mistake you can express.

**The file is an implicit `group`.** Bare `prova.test(...)`, `prova.flow(...)`, and
`prova.group(...)` at the top level register into it — so the terse common case (a handful of
independent tests) needs zero ceremony. And because the file defaults to the *independent*
(safe) strategy, **the presence of a `flow` is always the visible signal that ordering and
shared state are in play.** The dangerous axis is never silent; the safe axis is the quiet
default.

```lua
-- top level = the file's implicit group (independent)
prova.test("renders Cargo.toml", function(t) ... end)
prova.test("renders README",     function(t) ... end)   -- may run concurrently with the above

-- an explicit independent group, via its builder
prova.group("http surface", function(g)
  g:test("GET /health",  function(t) ... end)
  g:test("GET /version", function(t) ... end)
  g:flow("session lifecycle", function(f)               -- a flow nested in a group is one atomic unit
    f:step("login",  function(t) ... end)
    f:step("logout", function(t) ... end)
  end)
end)
```

### Units, isolation, and dependencies

A **unit** is the atom of scheduling: a top-level `test`, a `flow`, or a `group`. One uniform
rule governs how units relate:

> Units with no dependency edge between them are mutually isolated and may run in parallel
> (subject to resources). A dependency edge orders them and gates on success.

So **two flows with no edge between them run in parallel** — a flow is internally serial, but
flows are independent of *each other* unless you say otherwise. `prova.test`, `prova.flow`,
and `prova.group` all return handles; `depends_on` accepts any unit:

```lua
-- A login flow, then two independent journeys that both need a logged-in, populated account.
local login = prova.flow("login", function(f)
  f:step("authenticate", function(t) ... end)
end)

local populate = prova.flow("populate account", { depends_on = { login } }, function(f)
  f:step("seed profile", function(t) ... end)
  f:step("seed billing", function(t) ... end)
end)

-- Same upstreams, no edge between them → these two run in parallel.
prova.flow("checkout journey", { depends_on = { login, populate } }, function(f) ... end)
prova.flow("settings journey", { depends_on = { login, populate } }, function(f) ... end)
```

- The dependency graph is a DAG over units; a cycle is a collection-time error.
- If any upstream fails or is skipped, the dependent unit is **skipped, not failed**, with the
  reason (the TestNG behavior). A failed `login` skips `populate`, `checkout`, and `settings`
  — no cascade of spurious failures.
- `depends_on` gates on **pass/fail only; it does not transfer state.** Data flows through a
  fixture (or, within a linear sequence, the flow's own context). Keeping "did it pass?"
  separate from "give me its value" is deliberate — conflating them is how brittle implicit
  ordering creeps in.

### Flows — ordered steps with shared context

```lua
prova.flow("order lifecycle", function(f)
  local order                                 -- shared by all steps (the flow context)

  f:step("create", function(t)
    order = http.post(api .. "/orders", { json = { sku = "widget", qty = 2 } }):json()
    t:expect(order.id):is_truthy()
  end)

  f:step("read back", function(t)             -- skipped if "create" failed
    t:expect(http.get(api .. "/orders/" .. order.id):json().qty):equals(2)
  end)

  f:step("cancel", function(t)
    t:expect(http.post(api .. "/orders/" .. order.id .. "/cancel").status):equals(204)
  end)
end)
```

- A flow is **one scheduling unit**: steps run serially, in order, on one worker — never split.
- Flow-level fixtures via `f:use(fixture)` live for the flow's lifetime (`flow` scope). Inside
  a flow, `test`-scoped fixtures are per-**step**.
- A step may `t:skip(reason)` itself; a step is the flow's analog of a test for reporting.

### Concurrency is `--jobs`; it is never semantic

Because strategy is declared in the code, `--jobs N` sets only **how many workers may run** —
never what the tests *mean*. A flow is serial at `--jobs 100`; an independent group is
parallelizable at `--jobs 1` (it just won't actually overlap). The CLI cannot change
semantics, so it cannot surprise you.

Within the parallelizable set, shared *external* resources still need declaring so the
scheduler co-schedules safely:

```lua
prova.test("boots on :8080",  { resources = { "port:8080" } },        function(t) ... end)  -- exclusive
prova.test("reads shared db", { resources = { prova.shared("db") } }, function(t) ... end)  -- concurrent reader
```

Prefer the **typed constructors** over magic-format strings: `prova.port(8080)` and
`prova.resource("db")` instead of `"port:8080"`. A bare string is still accepted for ad-hoc
tokens and is **exclusive** by default; `prova.shared(x)` makes any resource a concurrent
reader; `{ serial = true }` is sugar for a process-wide exclusive. Declarations are inert at
`--jobs 1` and enforced above it, so you declare once and scale out without touching tests.

### Typed values vs. stringly-typed values

A deliberate stance on where strings are allowed, because "stringly-typed" is a classic
source of silent bugs:

- **Closed sets used inline stay string-literal *unions*** (`scope: "test"|"flow"|"file"|"suite"`,
  `order: "any"|"declared"`). LuaCATS types these as literal unions, so `lua-language-server`
  autocompletes the options and flags a typo at edit time — the enum benefit without the
  ceremony of importing a constants table, and the call site stays terse.
- **Magic-format strings become typed constructors.** A token like `"port:8080"` hides a
  `prefix:value` convention you can typo silently (`"prot:8080"` is a *valid but wrong*
  exclusive token). `prova.port(8080)` / `prova.resource("db")` can't be — and `port` can
  validate its number. Bare strings remain accepted for genuinely ad-hoc tokens.
- **Closed sets tied to real behavior are validated aliases.** `requires` takes
  `prova.Capability` (`"docker"|"network"|…`); a typo is rejected at collection time with a
  did-you-mean, not silently ignored (which would make a gate vanish).
- **Free-form sets stay strings.** `tags` and `ctx:skip(reason)` are open by definition.
- **Assertions were never stringly-typed** — matchers are methods (`:equals()`, `:contains()`),
  not `expect(x, "equals", y)`. Same principle, already applied.
- **Durations stay strings** (`"30s"`, `"500ms"`) — a well-known, readable micro-format — but
  are parsed strictly.

The cross-cutting rule underneath all of this: **every closed-set string is parsed into a Rust
enum at the mlua boundary and rejected at *collection time* with a helpful error** (valid
values / nearest match). The Lua-side literal-union types catch typos while editing; the
Rust-side parse guarantees a bad value fails loudly and early regardless of how it was written
— never a silent misbehavior at runtime.

### Order within a group, and hardening

A `group` makes **no order guarantee** — that contract is what keeps its children independent.
The runner may iterate in definition order for reproducibility, but you must not rely on it;
`prova test --shuffle[=seed]` randomizes (printing/reproducing the seed) to *prove*
independence. Ordered execution is never a group's job — that is what `flow` is for.

### Isolation — enforced by API shape, not sandboxing

"Hermetic by default" is honest here because the DSL exposes **no ambient mutable global
state**: no implicit cwd, no `os.chdir`; `shell.run(cmd, { cwd = …, env = … })` and
`fs.read(path)` take context explicitly; scratch is per-scope `ctx:tempdir()`. The *only*
sanctioned shared state is a flow's context (scoped, ordered, deterministically torn down) or
a broader-scoped fixture (tracked, reported). Two independent units therefore cannot corrupt
each other through the framework's own surface.

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
| `ctx:param()`         | fixtures | Current parameter (parametrized fixtures)                 |
| `t:expect(v, label?)` | tests    | Start a fluent assertion; optional `label` for messages   |
| `t:expect_all(fn)`    | tests    | Soft assertions — collect all failures in `fn`            |
| `t:skip(reason)`      | tests    | Skip the current test at runtime                          |
| `t.name`              | tests    | The resolved test name                                    |
| `t.case`              | tests    | The current case table (parametrized tests)               |

Everything callable on `t`/`ctx` is a colon-method (`t:expect`, `t:use`, `t:defer`) — no
dot/colon mix to trip over. The only dot members are plain data fields (`t.name`, `t.case`).

---

## Assertions

A single fluent entry point, `t:expect(subject, label?)`, returning a matcher. Matchers
validate the subject's type at call time, so domain subjects (file handles, shell results)
get rich checks.

```lua
t:expect(r.code):equals(0)
t:expect(r.stdout):contains("Compiling")
t:expect(r.stdout):matches("Finished .+ in %d")     -- Lua pattern
t:expect(out:file("Cargo.toml")):exists()
t:expect(out:dir("target")):never():exists()         -- negation
t:expect(res.status):is_one_of({ 200, 204 })
t:expect(value):is_nil()
t:expect(list):has_length(3)
```

The optional second argument is a **label** woven into the failure message, so anonymous
values still read clearly when they fail:

```lua
t:expect(order.id, "order id"):is_truthy()
-- on failure: "order id: expected a truthy value, got nil"
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

`:never()` returns a negated matcher (`t:expect(x):never():contains("secret")`).

### Soft assertions

By default a failed assertion **fails the test immediately**. `t:expect_all(function() … end)`
collects multiple failures before failing (useful for asserting many files at once):

```lua
t:expect_all(function()
  t:expect(out:file("README.md")):exists()
  t:expect(out:file("LICENSE")):exists()
  t:expect(out:file(".gitignore")):exists()
end)  -- reports every missing file, not just the first
```

### Snapshots

We already lean on Rust's `insta` in the archetect workspace; the file module exposes the
same idea to Lua for whole-file / whole-tree snapshotting:

```lua
t:expect(out:file("src/main.rs")):matches_snapshot()      -- .snap alongside the test file
t:expect(out:tree()):matches_snapshot("full-layout")      -- named snapshot of the rendered tree
```

`prova test --update-snapshots` rewrites them, mirroring `cargo insta`.

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
- Fixtures/helpers can live in a `conftest.lua`-equivalent (`prova.lua`) loaded before the
  test files in its directory and inherited by subdirectories.

```
$ prova test                     # discover & run under CWD
$ prova test tests/rust_cli      # a subtree
$ prova test -k "compiles"       # filter by name substring
$ prova test --tags net          # filter by tag
$ prova test --update-snapshots
$ prova test --jobs 8            # test-case-level parallelism (fixtures respect scope)
$ prova test --format tap|pretty|json
```

Two front doors over the same `prova-core` lib (mirrors `archetect-core` ← `archetect-bin`):
1. `prova` — the standalone binary (general-purpose positioning).
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
- **Annotations are the authoritative surface.** `library/prova.lua` + `library/modules.lua`
  (`---@meta` stubs) define the API; the runtime must conform to them. Risk to manage:
  drift between hand-written stubs and the Rust-registered API. For the POC we hand-maintain;
  longer term we may generate the stubs from the Rust registration to guarantee they match.
- **Generics carry fixture types.** `prova.fixture` → `prova.Fixture<T>`; `ctx:use(handle)`
  → `T`. This is *why* `use` takes a handle, not a string (see DI section).
- **Distribution mirrors `archetect ide setup`.** An `prova ide setup` command will
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
