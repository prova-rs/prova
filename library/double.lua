---@meta prova.double
--- `prova.double` — a transport-agnostic programmable **test double**: mock, proxy, or spy, with an
--- ordered event log. `local double = require("prova.double")`. The reusable heart of
--- `http.mock`/`grpc.mock`, for the boundary those cannot reach: an interaction a test drives
--- in-process from Lua (a plugin's effector, an injected dependency, any function-shaped seam).

--- Options for a double. `target` makes it a PROXY (unstubbed calls pass through to it and are
--- logged; stubs still win); omit it for a MOCK (an unstubbed call raises). `label` names the double
--- in that error.
---@class prova.DoubleOpts
---@field target? fun(input: any): any
---@field label? string

--- A programmable double. Callable — `d(input)` is `d:call(input)` — so it drops in where a real
--- function went. The module itself is the constructor: `require("prova.double")(opts?)` (or its
--- `.new(opts?)` alias) returns one.
---@class prova.Double
---@overload fun(opts?: prova.DoubleOpts): prova.Double
local Double = {}

--- Create a double (alias for calling the module directly).
---@param opts? prova.DoubleOpts
---@return prova.Double
function Double.new(opts) end

--- Register a stub; chain `:reply(…)` onto the returned handle. First matching stub wins, in
--- declaration order. `match` is a SUBSET table (each field must deep-equal in the input), a
--- predicate function, or nil (matches every call).
---@param match table|fun(input: any): boolean|nil
---@return prova.DoubleStub
function Double:on(match) end

--- Dispatch one call: record it (with a monotonic `seq`), then answer it — a stub, else a proxy
--- `target`, else raise (a mock's unpredicted call is a finding). `d(input)` is sugar for this.
---@param input any
---@return any reply
function Double:call(input) end

--- Every call the double saw, IN ORDER, as data: `{ seq, input, reply, matched, source }` where
--- `source` is "stub" | "target" | "unmatched". `filter` narrows over the recorded `input` the same
--- way `:on` matches — so you assert on a subset, and on ordering, with the ordinary matchers.
---@param filter? table|fun(input: any): boolean
---@return prova.DoubleCall[]
function Double:received(filter) end

--- Discard the recorded calls (not the stubs), to assert on a test's phases in isolation.
function Double:reset() end

--- A stub handle from `Double:on`. `:reply(…)` sets its answer.
---@class prova.DoubleStub
local DoubleStub = {}

--- Answer matching calls with `reply` — a value, or a `function(input)` computing one. The function
--- is real Lua, run when the call arrives, so it computes from the input and closes over test locals.
---@param reply any|fun(input: any): any
---@return prova.DoubleStub
function DoubleStub:reply(reply) end

--- One recorded call. A handler's argument and a journal row are the same shape.
---@class prova.DoubleCall
---@field seq integer      # 1-based position in the log — monotonic, so ordering is assertable
---@field input any        # the argument the double was called with
---@field reply any        # what it answered (absent for an unmatched call)
---@field matched boolean  # whether a stub matched
---@field source string    # "stub" | "target" (proxied) | "unmatched"
