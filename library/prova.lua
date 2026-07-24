---@meta
--- Prova — LuaLS annotations for the test/fixture DSL.
---
--- This file is authoritative for the authoring surface: it drives editor completion,
--- hover, and type-checking (lua-language-server). It is a `---@meta` stub — no runtime
--- behavior lives here. Ship it in the data dir alongside archetect's own annotations so
--- editors resolve `require("prova")` against it.

------------------------------------------------------------------------------------------
-- Contexts
------------------------------------------------------------------------------------------

---Base context passed to fixture factories.
---@class prova.Context
---@field network? prova.Network   # the topology's managed network — non-nil ONLY inside a `prova.topology` factory; resources provisioned there via `prova.containerized` auto-join it, aliased by name. Nil in ordinary fixtures and test bodies.
local Context = {}

---Instantiate or fetch a fixture value. Lazy: the fixture is built on first use and then
---cached for its scope. Fixture-to-fixture dependencies use this too.
---
---Prefer passing the **handle** returned by `prova.fixture` — the fixture's value type
---then flows through to the call site (full completion + type-checking). Passing a bare
---string name also works (cross-file lookup) but yields an untyped `any`.
---@generic T
---@param fixture prova.Fixture<T>   # handle from prova.fixture — type flows through
---@return T
---@overload fun(self: prova.Context, name: string): any
function Context:use(fixture) end

---Register a teardown callback for the current scope. Callbacks run LIFO when the scope
---ends; a fixture's deferrals run before those of any fixture it depended on.
---@param fn fun()
function Context:defer(fn) end

---Tie a resource's lifecycle to the current scope: on teardown, call its `stop()` (containers,
---processes) or `close()` (connections). Returns the resource, so it composes inline:
---`local pg = ctx:manage(docker.run{...})`. Sugar over `defer` — use `defer` for custom teardown.
---@generic T
---@param resource T
---@return T
function Context:manage(resource) end

---Create a scratch directory that is removed automatically when the current scope ends.
---@return string path
function Context:tempdir() end

---Attach a structured log line to the current test/fixture in the report.
---@param msg string
function Context:log(msg) end

---Context passed to test bodies. Extends `prova.Context` with assertions and control flow.
---@class prova.TestContext : prova.Context
---@field name string                              # resolved test name
---@field case table|nil                           # current case (parametrized tests)
local TestContext = {}

---Start a fluent assertion. The optional `label` is used in the failure message, so a failed
---check reads e.g. "order id: expected truthy, got nil" instead of pointing at an anonymous
---value.
---@param subject any
---@param label? string
---@return prova.Matcher
function TestContext:expect(subject, label) end

---Soft assertions: collect every failure inside `body` before failing the test. Reports all
---failures, not just the first.
---@param body fun()
function TestContext:expect_all(body) end

---Skip the current test at runtime with a reason.
---@param reason string
function TestContext:skip(reason) end

------------------------------------------------------------------------------------------
-- Fixtures
------------------------------------------------------------------------------------------

---A fixture handle returned by `prova.fixture`. Generic over the fixture's value type `T`,
---so `ctx:use(handle)` recovers `T` at the call site. Treat it as opaque — pass it to
---`use`, don't inspect it.
---@class prova.Fixture<T>

------------------------------------------------------------------------------------------
-- Matchers
------------------------------------------------------------------------------------------

---Fluent assertion matcher returned by `expect(subject)`. Matchers validate the subject's
---type at call time, so filesystem/shell/http subjects get domain-specific checks.
---@class prova.Matcher
local Matcher = {}

---Return a negated matcher: `expect(x):never():contains("secret")`.
---@return prova.Matcher
function Matcher:never() end

--- Deep structural equality (recurses into tables).
---@param x any
function Matcher:equals(x) end
--- Alias for `equals` (deep structural equality).
---@param x any
function Matcher:eq(x) end
--- Identity: the *same* table/function/userdata (by reference), or an equal primitive (`rawequal`).
--- Use over `equals` when you mean "the same object" — including tables with function fields that
--- deep-equals cannot compare.
---@param x any
function Matcher:is(x) end
--- Passes for anything except `false`/`nil` — Lua truthiness, so `0` and `""` PASS.
function Matcher:is_truthy() end
--- Passes for `false` or `nil` only — `0` and `""` are truthy in Lua and FAIL here.
function Matcher:is_falsy() end
--- Strictly the boolean `true` (not merely truthy).
function Matcher:is_true() end
--- Strictly the boolean `false` (not merely falsy — `nil` fails).
function Matcher:is_false() end
--- The subject is `nil`.
function Matcher:is_nil() end
---Substring (strings) or membership (tables).
---@param x any
function Matcher:contains(x) end
---Lua-pattern match (strings).
---@param pattern string
function Matcher:matches(pattern) end
--- `#subject == n` — a string's byte length or a sequence's element count.
---@param n integer
function Matcher:has_length(n) end
--- The subject equals one of `options` (deep equality).
---@param options any[]
function Matcher:is_one_of(options) end
--- Numeric `subject > n`.
---@param n number
function Matcher:gt(n) end
--- Numeric `subject >= n`.
---@param n number
function Matcher:gte(n) end
--- Numeric `subject < n`.
---@param n number
function Matcher:lt(n) end
--- Numeric `subject <= n`.
---@param n number
function Matcher:lte(n) end
--- Filesystem-handle matchers (subject is a file/dir/tree handle):
--- The path exists (file or directory).
function Matcher:exists() end
--- The path exists and is a regular file.
function Matcher:is_file() end
--- The path exists and is a directory.
function Matcher:is_dir() end
--- Empty for whatever the subject is: a string with no bytes, a table with no entries, or a path
--- with no children (a directory with no entries, a file of zero bytes).
function Matcher:is_empty() end
--- Assert a rendered tree (a tree/dir handle, or a path string) has no leftover template markers —
--- no `{{`, `{%`, or `{#` in any file's contents or path segments. GitHub Actions `${{ … }}`
--- expressions are excluded. The signature archetype check.
function Matcher:is_fully_rendered() end
---Compare the subject against a stored `.snap` file colocated with the test
---(`snapshots/<file>__<key>.snap`). Pass `prova --update-snapshots` (`-u`) to (re)write it; a
---mismatch fails with a line diff, a missing snapshot writes a reviewable `.snap.new`.
---
---The subject may be a **string** (compared as-is) or a **path handle** — any table with a `path`
---field, e.g. `archetect.render` output or `out:file(rel)`. For a path handle, `level` selects how
---much is captured: `"layout"` (sorted relative paths — the render's shape; the default for a
---directory) or `"content"` (paths + each file's bytes). Keep `content` snapshots narrow.
---@param opts? string|{ name?: string, level?: "layout"|"content" }  # a name, or options
function Matcher:matches_snapshot(opts) end

------------------------------------------------------------------------------------------
-- Registration API
------------------------------------------------------------------------------------------

--- Reserved for future fixture options. Scope is a `Scope` value, not an option.
--- (Parametrized fixtures were considered and deliberately dropped — parametrization stays explicit:
--- `test_each` for data-driven tests, separate suites for divergent variants, profiles for env.)
---@class prova.FixtureOpts
---@field autouse? boolean               # run even when no test names it

--- An opaque, typed resource reference from `prova.port`/`prova.resource`/`prova.shared`.
--- Prefer these constructors over magic-format strings like `"port:8080"` — the prefix in a
--- bare string is a convention you can typo silently; a constructor cannot be.
---@class prova.ResourceRef
--- What a `resources` list accepts: a typed ref, or a bare string token for ad-hoc names.
--- Bare strings are exclusive by default; wrap with `prova.shared` for a concurrent reader.
---@alias prova.Resource prova.ResourceRef|string

--- A capability a unit requires to run. Missing capability → the unit is SKIPPED (with a
--- reason), never failed. A closed, validated set (plugins may register more); a typo like
--- `"dcoker"` is rejected at collection time rather than silently ignored.
--- A capability expression: a name, optionally with a semver constraint.
---
---   "docker"            -- the daemon answers, and runs LINUX containers
---   "dotnet >= 9"       -- present AND new enough; SDK 8 skips instead of dying at build time
---   "node ^20"          -- any semver operator; whitespace is not significant
---   "git >= 1.0, < 3.0" -- ranges work
---   "unix" / "windows"  -- platform predicates (no version: `cfg!` has no number)
---
--- Unknown names fall through to a binary-on-PATH probe, so `requires = { "kubectl" }` needs no
--- registration. A version is probed from the tool itself (`--version`; docker reports its SERVER
--- version, which is the thing a suite depends on) and padded to three components.
---
--- Unmet → the unit SKIPS, with a reason that says which of the three things went wrong: absent,
--- too old, or a malformed expression (that one is an error — a constraint that can never parse
--- would skip forever and read as green). The same expression grammar is what a profile's
--- `must_run` takes in prova.toml, where an unmet one FAILS instead: one vocabulary, two directions.
---@alias prova.Capability "docker"|"network"|"git"|"github"|"unix"|"windows"|string

--- A handle to any schedulable unit — a `test`, `flow`, or `group`. Pass to `depends_on`.
--- Units with no edge between them are mutually isolated and may run in parallel.
---@alias prova.Unit prova.Test|prova.Flow|prova.Group

--- A test handle returned by `prova.test`/`prova.test_each`. Pass to `depends_on`.
---@class prova.Test
--- A flow handle returned by `prova.flow`. One ordered scheduling unit.
---@class prova.Flow
--- A group handle returned by `prova.group`. One scheduling unit whose children run per the
--- group's independent strategy.
---@class prova.Group

--- Options shared by any schedulable unit (test/flow/group).
---@class prova.UnitOpts
---@field tags? string[]                 # selection tags (select with `--tags a,b`; `!tag` excludes), free-form
---@field requires? prova.Capability[]   # skip (not fail) if a capability is unavailable
---@field depends_on? prova.Unit[]       # skip this unit if any upstream failed/was skipped
---@field resources? prova.Resource[]    # resources this unit needs (concurrency gating)
---@field serial? boolean                # never run concurrently with anything (process-wide exclusive)

---@class prova.TestOpts : prova.UnitOpts
---@field timeout? string                # e.g. "30s"
---@field retries? integer
---@field spec? true|string              # a proof authored AHEAD of its implementation (value = reason/ticket). Open spec (red body) reports as its own outcome, CI green; once the body passes, the run fails until this flag is deleted — implementation and flag removal land as one commit. Test/flow-level only. See `prova learn specs`.

---@class prova.FlowOpts : prova.UnitOpts
---@field timeout? string                # whole-flow timeout
---@field spec? true|string              # a proof authored AHEAD of its implementation — see prova.TestOpts.spec / `prova learn specs`

--- A group is the *independent* strategy: children are isolated, unordered, parallelizable.
---@class prova.GroupOpts : prova.UnitOpts
---@field order? "any"|"declared"        # default "any" — do not rely on order; use `flow` if you need it
---@field parallel? boolean              # default true — set false to serialize the group's children

--- The flow builder: the *sequence* strategy. Declares ordered steps that share the flow's
--- scope; later steps are skipped once an earlier one fails. Shared mutable state lives here
--- and only here — this is the sole capability that grants it.
---@class prova.FlowBuilder
local FlowBuilder = {}
--- Declare an ordered step. Steps run in declaration order on a single worker.
--- Fixtures are used *inside* a step via `t:use(...)`, like everywhere else — a flow-scoped fixture
--- is scope-cached, so `t:use(f)` returns the same instance across the flow's steps. (There is
--- deliberately no `flow:use` builder sugar; see docs/plans/phase1-ergonomics.md.)
---@param name string
---@param body fun(t: prova.TestContext)
function FlowBuilder:step(name, body) end

--- The group builder: the *independent* strategy. Declares child units (tests, flows, nested
--- groups) that are isolated and parallelizable. It deliberately exposes **no shared-state
--- mechanism** — cross-child built-up context is not representable here; use a `flow`.
---@class prova.GroupBuilder
local GroupBuilder = {}
--- Declare an independent test in this group.
---@overload fun(self: prova.GroupBuilder, name: string, factory: fun(t: prova.TestContext)): prova.Test
---@param name string
---@param opts prova.TestOpts
---@param factory fun(t: prova.TestContext)
---@return prova.Test
function GroupBuilder:test(name, opts, factory) end
--- Table-driven tests within this group: one test per case, names filled from `{placeholder}`s.
---@param name_template string
---@param cases table[]
---@param factory fun(t: prova.TestContext, case: table)
---@return prova.Test[]
function GroupBuilder:test_each(name_template, cases, factory) end
--- Declare a flow (ordered sequence) as a child unit of this group.
---@overload fun(self: prova.GroupBuilder, name: string, body: fun(flow: prova.FlowBuilder)): prova.Flow
---@param name string
---@param opts prova.FlowOpts
---@param body fun(flow: prova.FlowBuilder)
---@return prova.Flow
function GroupBuilder:flow(name, opts, body) end
--- Declare a nested group.
---@overload fun(self: prova.GroupBuilder, name: string, body: fun(g: prova.GroupBuilder)): prova.Group
---@param name string
---@param opts prova.GroupOpts
---@param body fun(g: prova.GroupBuilder)
---@return prova.Group
function GroupBuilder:group(name, opts, body) end
--- Label-only subgrouping for reporting (inherits strategy; no new scope).
---@param label string
---@param body fun(g: prova.GroupBuilder)
function GroupBuilder:describe(label, body) end

-- There are deliberately NO before_each/after_each/before_all/after_all hooks, on groups or
-- files. Setup that produces a value is a `prova.fixture` (lazy, cached, scope-torn-down);
-- teardown that belongs to a value is `ctx:manage`/`ctx:defer` (guaranteed, LIFO). Fixtures
-- hold setup and teardown together — that is the model.

--- The `prova` table is **injected as a global by the runtime** — no `require` needed, just
--- like the `fs`/`shell`/`http`/`archetect` modules. `require("prova")` is still supported
--- (and returns this same table) for anyone who prefers an explicit import.
---@class prova
---@field root string   # the PROJECT root — the ancestor dir the manifest was found under (the repo). Anchor repo paths here: `prova.root .. "/target/debug/app"`. Nil with no manifest.
---@field home string   # where `prova.toml` lives — the root, or its `prova/` / `.prova/` child. Anchor manifest-relative paths here. Nil with no manifest.
---@field ports string  # host port mode: "auto" | "fixed"
prova = {}

--- The API surface, as data — every function, method, and value shape as
--- `{ name, signature, summary }`. `filter` narrows by case-insensitive substring across name and
--- summary. Ask this instead of guessing: `prova.help("shell")`, `prova.help("ShellResult")`.
--- Parsed from these same stubs, so it cannot drift from what your editor completes.
---@param filter? string
---@return { name: string, signature: string, summary: string }[]
function prova.help(filter) end

---Declare a fixture: a named factory producing a value, with scoped teardown and dependencies.
---`scope` is a `Scope` value (`Scope.Test`/`Scope.Flow`/`Scope.File`/`Scope.Suite`); omit it for
---`Scope.Test`. Returns a typed handle; pass it to `ctx:use(handle)` so the value type flows through.
---@generic T
---@overload fun(name: string, factory: fun(ctx: prova.Context): T): prova.Fixture<T>
---@param name string
---@param scope prova.ScopeRef
---@param factory fun(ctx: prova.Context): T
---@param opts? prova.FixtureOpts   # reserved for parametrization (not yet implemented)
---@return prova.Fixture<T>
function prova.fixture(name, scope, factory, opts) end

---Declare a **topology**: a named, verb-agnostic bundle of wired resources. It is a fixture (default
---`Scope.File` — provisioned once, shared across a file's tests) that is *also* addressable by name:
---`prova up <name>` and `prova start <name>` stand up the identical object outside any test, printing
---each resource's `url`. In test mode use it exactly like any fixture — `t:use(handle)`. One
---definition powers both your tests and your dev environment, so they cannot drift.
---@generic T
---@overload fun(name: string, factory: fun(ctx: prova.Context): T): prova.Fixture<T>
---@param name string
---@param scope prova.ScopeRef
---@param factory fun(ctx: prova.Context): T
---@return prova.Fixture<T>
function prova.topology(name, scope, factory) end

--- An opaque fixture-scope value — a member of the `Scope` global.
---@class prova.ScopeRef
---@field scope string   # the scope name ("test"|"flow"|"file"|"suite")

--- Typed fixture-scope constants (the `scope` argument to `prova.fixture`):
---  * `Scope.Test`  — rebuilt fresh for each test (the default).
---  * `Scope.Flow`  — built once per `prova.flow`, shared across its steps.
---  * `Scope.File`  — built once per file, shared across the file's tests.
---  * `Scope.Suite` — built once per suite (a group of files sharing one state; see `suite.lua`).
---@class prova.Scope
---@field Test prova.ScopeRef
---@field Flow prova.ScopeRef
---@field File prova.ScopeRef
---@field Suite prova.ScopeRef
Scope = {}

---@class prova.SuiteConfig
---@field name? string          # display name for the suite (default: the directory name)
---@field requires? string[]    # capabilities gating the whole suite — unmet → every file skips

--- Configure the current suite — call in a `suite.lua` file (a directory's `suite.lua` groups its
--- `*_test.lua` into one suite that shares a Lua state, so `Scope.Suite` fixtures are built once and
--- shared across the files). Test files reference the suite's fixtures by name, e.g. `t:use("db")`.
---@class prova.suite
suite = {}
--- Declare this suite's configuration (see `prova.SuiteConfig`). Call once, at the top of a file.
---@param config prova.SuiteConfig
function suite.config(config) end

-- The top-level `prova.test`/`test_each`/`flow`/`group` register into the file's implicit
-- group (the independent strategy). Inside an explicit group, use the `GroupBuilder` methods.

---Declare an independent test in the file's implicit group. Returns a handle for `depends_on`.
---@overload fun(name: string, factory: fun(t: prova.TestContext)): prova.Test
---@param name string
---@param opts prova.TestOpts
---@param factory fun(t: prova.TestContext)
---@return prova.Test
function prova.test(name, opts, factory) end

---Declare a table-driven test: one test per case. `{placeholder}`s in `name_template` are filled
---from each case table; the case reaches the body as its second argument and as `t.case`. Returns
---the list of generated test handles (any usable in `depends_on`).
---@param name_template string
---@param cases table[]
---@param factory fun(t: prova.TestContext, case: table)
---@return prova.Test[]
function prova.test_each(name_template, cases, factory) end

---Declare a flow: an ordered sequence of steps sharing the flow's scope. Steps run in order
---on one worker; once a step fails, the rest are skipped. The go-to construct when you need
---ordering plus built-up state. Returns a handle usable in `depends_on`.
---@overload fun(name: string, body: fun(flow: prova.FlowBuilder)): prova.Flow
---@param name string
---@param opts prova.FlowOpts            # tags/resources/depends_on apply to the whole flow
---@param body fun(flow: prova.FlowBuilder)
---@return prova.Flow
function prova.flow(name, opts, body) end

---Declare an independent group: an isolated, unordered, parallelizable bag of child units.
---The builder exposes `test`/`flow`/`group` but **no shared-state mechanism** — that is the
---point (invalid states unrepresentable). Returns a handle usable in `depends_on`.
---@overload fun(name: string, body: fun(g: prova.GroupBuilder)): prova.Group
---@param name string
---@param opts prova.GroupOpts
---@param body fun(g: prova.GroupBuilder)
---@return prova.Group
function prova.group(name, opts, body) end

---A typed **exclusive** resource for a TCP port. Preferred over `"port:8080"` — validates the
---number and can't be mistyped into an unrelated token.
---@param number integer
---@return prova.ResourceRef
function prova.port(number) end

---A typed **exclusive** resource for an arbitrary named token (a DB, an account, a path).
---@param token string
---@return prova.ResourceRef
function prova.resource(token) end

---Mark a resource as a **concurrent reader** (readers-writer semantics): readers run together,
---but an exclusive holder waits for all readers to release. Accepts a typed ref or a bare
---string token.
---@param resource prova.ResourceRef|string
---@return prova.ResourceRef
function prova.shared(resource) end

---Group tests for reporting. Labeling-only in v1 (does not introduce a new fixture scope).
---@param label string
---@param body fun()
function prova.describe(label, body) end

---Await `millis` milliseconds without blocking the worker (cooperative async). A low-level timing
---primitive, mainly for tests and boot-then-probe waits; prefer `http.wait_for` for readiness polls.
---@param millis integer
function prova.sleep(millis) end

---The exec-CLI output-parsing toolkit: turn the text a container CLI returns into Lua values.
---@class prova.parse
prova.parse = {}

---Split into non-empty, trimmed lines.
---@param s string
---@return string[]
function prova.parse.lines(s) end

---Split each non-empty line on `sep` (default tab) into a list of columns.
---@param s string
---@param sep? string
---@return string[][]
function prova.parse.rows(s, sep) end

---Treat the first non-empty line as a header row; return each remaining row as a map keyed by header.
---@param s string
---@param sep? string
---@return table<string, string>[]
function prova.parse.table(s, sep) end

---@class prova.RetryOpts
---@field timeout? string    # overall deadline (default "30s")
---@field every? string      # interval between attempts (default "500ms")
---@field message? string    # error message on timeout

---Call `fn` repeatedly until it returns a truthy value (a raised error counts as "not ready"), or
---the deadline elapses. Returns the value. The readiness primitive — replaces the hand-rolled
---`for _=1,N do pcall(...) sleep end` loop: `local conn = prova.retry(function() return sqlite.client(url) end)`.
---@generic T
---@param fn fun(): T
---@param opts? prova.RetryOpts
---@return T
function prova.retry(fn, opts) end

---The address a resource is reached at from a given vantage. The host vantage (`resource.url`/
---`host`/`port`) is the mapped port a host process — the test runner — uses; the network vantage
---(`resource.network`) is the alias + container port an in-network consumer — a containerized SUT
---joined to the same topology network — uses.
---@class prova.NetworkVantage
---@field url string      # connection URL at the network authority (alias:container_port)
---@field host string     # the network alias (DNS name on the topology network)
---@field port integer    # the CONTAINER port (not the mapped host port)
---@field alias string    # the alias this resource answers to

---The standard resource shape a containerized recipe/plugin returns.
---@class prova.ContainerResource
---@field url string                    # host-vantage connection URL (127.0.0.1:mapped_port)
---@field host string                   # "127.0.0.1"
---@field port integer                  # the mapped host port
---@field container any                 # the docker Container handle
---@field client? any                   # the attached client (when the spec provides one)
---@field network? prova.NetworkVantage # present when provisioned on a network (opts.network + opts.alias)

---@class prova.ContainerizedSpec
---@field name? string                         # namespace name, for error messages
---@field image? string                        # base image repo (e.g. "redis") — PULLED; `opts.image` fully overrides. Mutually exclusive with `build`
---@field build? string|prova.DockerBuildOpts  # BUILT from a Dockerfile (the system under test) — a string is shorthand for `{ context = … }`. Mutually exclusive with `image`
---@field tag? string                          # default tag for a pulled `image`; `opts.tag` overrides
---@field port? integer                        # primary published port (readiness + url); or use `ports`
---@field ports? integer|(integer|{ container: integer, host: integer })[]  # ports to publish; a `{container,host}` entry fixes the host port
---@field command? string                      # optional container command
---@field env? table<string,string>|fun(opts: table): table<string,string>  # container env (may read opts)
---@field wait? { port?: integer, log?: string }  # readiness probe (default: primary port)
---@field timeout? string                      # readiness deadline (default "60s")
---@field url fun(host_port: integer, opts: table): string  # connection URL from the mapped host port
---@field client? fun(url: string, opts: table, container: any): any  # attach a client (native uses url; docker-exec uses container); omit for black-box
---@field extra? fun(url: string, opts: table, container: any): table  # additional resource fields beyond the trio (e.g. s3 credentials)

---Build a grammar-conformant resource namespace (`{ client?, container }`) from a compact spec — the
---scaffolding every containerized recipe/plugin is authored through, so first-party and third-party
---resources come out the same shape (the tier-agnostic interface). The generated
---`container(ctx, opts?)` provisions via docker, waits, ties teardown to the scope, and returns a
---`prova.ContainerResource`, attaching a managed `client` only when the spec provides a `client`
---factory. `opts.network` (a `docker.network()` handle or name) + `opts.alias` join the resource to
---a topology network and populate `resource.network` — how a containerized SUT reaches it.
---
---The same recipe expresses the **system under test**: give it `build` instead of `image` and the
---image is built from the project's own Dockerfile rather than pulled. A SUT is not a separate
---concept — it is a resource whose image is local, so it inherits the topology auto-join, the
---network vantage, readiness and teardown unchanged, and the suite needs only Docker (no host SDK).
---@param spec prova.ContainerizedSpec
---@return { client?: fun(url: string, opts: table): any, container: fun(ctx: prova.Context, opts?: table): prova.ContainerResource }
function prova.containerized(spec) end

--- `runtime.*` — the Lua-shaped configuration DSL for the whole runtime. Available ONLY in the
--- optional `prova.lua` companion beside `prova.toml`; calling it from a test raises (it configures
--- the environment tests run *in*, and only the companion loads early enough to do that).
---@class prova.runtime
runtime = {}

--- Register a project-wide capability — a fact about what THIS system, as configured, can test: its
--- OS, hardware, and software. A property of the environment, not the code. Built-ins cover the
--- common cases (`unix`/`windows`, `docker`, `dotnet >= 9`, any tool on PATH); this is the escape
--- hatch for the rest — a GPU, a kind cluster, a licence file. The name (not the closure) is what
--- lets a skip say what was missing; the predicate is only how it is detected. Once registered the
--- name works in both directions: `requires = { "gpu" }` (skip if unmet) and `must_run = ["gpu"]`
--- (fail if unmet). See docs/design/test-topology.md.
---@param name string                          # the capability name used in `requires` / `must_run`
---@param predicate fun(): boolean|string|nil  # true = available · a version string = comparable (`gpu >= 2`) · false/nil = unavailable
function runtime.capability(name, predicate) end

return prova
