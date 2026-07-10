---@meta
--- Assay — LuaLS annotations for the test/fixture DSL.
---
--- This file is authoritative for the authoring surface: it drives editor completion,
--- hover, and type-checking (lua-language-server). It is a `---@meta` stub — no runtime
--- behavior lives here. Ship it in the data dir alongside archetect's own annotations so
--- editors resolve `require("assay")` against it.

------------------------------------------------------------------------------------------
-- Contexts
------------------------------------------------------------------------------------------

---Base context passed to fixture factories.
---@class assay.Context
local Context = {}

---Instantiate or fetch a fixture value. Lazy: the fixture is built on first use and then
---cached for its scope. Fixture-to-fixture dependencies use this too.
---@param name string
---@return any value
function Context:use(name) end

---Register a teardown callback for the current scope. Callbacks run LIFO when the scope
---ends; a fixture's deferrals run before those of any fixture it depended on.
---@param fn fun()
function Context:defer(fn) end

---Create a scratch directory that is removed automatically when the current scope ends.
---@return string path
function Context:tempdir() end

---Attach a structured log line to the current test/fixture in the report.
---@param msg string
function Context:log(msg) end

---Current parameter value for a parametrized fixture (see `params` on `assay.fixture`).
---@return any param
function Context:param() end

---Context passed to test bodies. Extends `assay.Context` with assertions and control flow.
---@class assay.TestContext : assay.Context
---@field expect fun(subject: any): assay.Matcher  # start a fluent assertion
---@field name string                              # resolved test name
---@field case table|nil                           # current case (parametrized tests)
local TestContext = {}

---Collect multiple assertion failures inside `body` before failing the test (soft
---assertions). Reports every failure, not just the first.
---@param body fun()
function TestContext:expect_all(body) end

---Skip the current test at runtime with a reason.
---@param reason string
function TestContext:skip(reason) end

------------------------------------------------------------------------------------------
-- Matchers
------------------------------------------------------------------------------------------

---Fluent assertion matcher returned by `expect(subject)`. Matchers validate the subject's
---type at call time, so filesystem/shell/http subjects get domain-specific checks.
---@class assay.Matcher
local Matcher = {}

---Return a negated matcher: `expect(x):never():contains("secret")`.
---@return assay.Matcher
function Matcher:never() end

---@param x any
function Matcher:equals(x) end
---@param x any
function Matcher:eq(x) end
function Matcher:is_truthy() end
function Matcher:is_falsy() end
function Matcher:is_true() end
function Matcher:is_false() end
function Matcher:is_nil() end
---Substring (strings) or membership (tables).
---@param x any
function Matcher:contains(x) end
---Lua-pattern match (strings).
---@param pattern string
function Matcher:matches(pattern) end
---@param n integer
function Matcher:has_length(n) end
---@param options any[]
function Matcher:is_one_of(options) end
---@param n number
function Matcher:gt(n) end
---@param n number
function Matcher:gte(n) end
---@param n number
function Matcher:lt(n) end
---@param n number
function Matcher:lte(n) end
--- Filesystem-handle matchers (subject is a file/dir/tree handle):
function Matcher:exists() end
function Matcher:is_file() end
function Matcher:is_dir() end
function Matcher:is_empty() end
---Compare against a stored snapshot (`assay test --update-snapshots` to rewrite).
---@param name? string  # optional named snapshot
function Matcher:matches_snapshot(name) end

------------------------------------------------------------------------------------------
-- Registration API
------------------------------------------------------------------------------------------

---@class assay.FixtureOpts
---@field scope? "test"|"file"|"suite"   # default "test"
---@field autouse? boolean               # run even when no test names it
---@field params? any[]                  # parametrize: one variant per element (see Context:param)

---@class assay.TestOpts
---@field timeout? string                # e.g. "30s"
---@field retries? integer
---@field tags? string[]

---@class assay
local assay = {}

---Declare a fixture: a named factory producing a value, with optional scoped teardown and
---dependencies. `scope` may be a string ("test"|"file"|"suite") or a full options table.
---@param name string
---@param scope "test"|"file"|"suite"|assay.FixtureOpts
---@param factory fun(ctx: assay.Context): any
---@param opts? assay.FixtureOpts        # when `scope` is a bare string, extra opts (e.g. params)
function assay.fixture(name, scope, factory, opts) end

---Declare a test.
---@overload fun(name: string, factory: fun(t: assay.TestContext))
---@param name string
---@param opts assay.TestOpts
---@param factory fun(t: assay.TestContext)
function assay.test(name, opts, factory) end

---Declare a table-driven test: one test per case. `{placeholders}` in `name_template` are
---filled from each case table.
---@param name_template string
---@param cases table[]
---@param factory fun(t: assay.TestContext, case: table)
function assay.test_each(name_template, cases, factory) end

---Group tests for reporting. Labeling-only in v1 (does not introduce a new fixture scope).
---@param label string
---@param body fun()
function assay.describe(label, body) end

---@param fn fun(t: assay.TestContext)
function assay.before_each(fn) end
---@param fn fun(t: assay.TestContext)
function assay.after_each(fn) end
---@param fn fun()
function assay.before_all(fn) end
---@param fn fun()
function assay.after_all(fn) end

return assay
