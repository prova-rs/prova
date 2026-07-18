-- prova.double — a transport-agnostic programmable **test double**: mock, proxy, or spy, with an
-- ordered event log. The reusable heart of `http.mock`/`grpc.mock`, for the boundary those two
-- cannot reach: an interaction a test drives **in-process from Lua** — a plugin's effector, an
-- injected dependency, any function-shaped seam. No server, no port, no protocol.
--
-- A bundled first-party plugin, loaded through the same searcher user plugins use. Pure Lua over
-- tables and closures — there is nothing here that needs to be native, and a native version would be
-- ceremony around logic (Prova's "don't add a primitive when a recipe suffices").
--
-- ## One object, three roles — the role is which knobs you set, not a different type
--
--   * **mock**  — stub replies; an unstubbed call is loud (it is the most interesting thing a double
--                 can tell you): `local d = double()`
--   * **proxy** — pass calls through to a real target and LOG them, stubbing only what you choose to
--                 override; stubs always win: `local d = double{ target = real_fn }`
--   * **spy**   — a proxy that overrides nothing: `local d = double{ target = real_fn }` with no
--                 stubs is exactly a logging pass-through.
--
-- ## The grammar is the mocks' grammar
--
--   local ps = double()                                   -- or double{ target = send }
--   ps:on{ method = "batchPlay" }:reply(function(call) return { ok = true } end)
--   ps:on{ method = "close" }:reply({ ok = true })        -- a value, or a function of the call
--
--   -- hand `ps` where a real callable went; the system under test just calls it:
--   ps{ method = "batchPlay", commands = { … } }          -- ps(input) == ps:call(input)
--
--   -- then assert on the ORDERED log — call ordering falls out of it:
--   local calls = ps:received{ method = "batchPlay" }
--   t:expect(#calls):equals(1)
--   t:expect(ps:received()[1].input.method):equals("batchPlay")   -- what came first
--
-- `received()` returns DATA (plain tables), for the ordinary matchers — there is no verify-DSL,
-- because `t:expect` already exists. This mirrors `MockServer:received` exactly.

local M = {}

-- Deep structural equality, so a subset match (`{ method = "x" }`) can compare against a nested
-- input value the same way `expect(x):equals(y)` does.
local function deep_equal(a, b)
  if a == b then return true end
  if type(a) ~= "table" or type(b) ~= "table" then return false end
  for k, v in pairs(a) do
    if not deep_equal(v, b[k]) then return false end
  end
  for k in pairs(b) do
    if a[k] == nil then return false end
  end
  return true
end

-- Does `input` satisfy `match`? A **function** is an arbitrary predicate; a **table** is a SUBSET
-- match (every field it names must deep-equal in the input, others unconstrained — like `MockMatch`);
-- anything else (or nil) matches everything.
local function matches(match, input)
  if type(match) == "function" then return match(input) and true or false end
  if type(match) ~= "table" then return true end
  if type(input) ~= "table" then return false end
  for k, v in pairs(match) do
    if not deep_equal(input[k], v) then return false end
  end
  return true
end

-- A shallow copy so a caller cannot mutate the double's own journal by holding a returned row.
local function shallow(t)
  local c = {}
  for k, v in pairs(t) do c[k] = v end
  return c
end

local Double = {}

--- Register a stub. Returns a handle whose `:reply(…)` sets the answer, so it chains:
--- `d:on{…}:reply(…)`. First matching stub wins, in declaration order (as in the mocks).
--- @param match table|fun(input): boolean|nil  # subset table, predicate, or nil (matches all)
--- @return table  # a stub handle with `:reply(reply)`
function Double:on(match)
  local stub = { match = match, reply = nil, has_reply = false }
  self._stubs[#self._stubs + 1] = stub
  local handle = {}
  --- Answer matching calls with `reply` — a value, or a `function(input)` computing one. The
  --- function is real Lua, run when the call arrives, so it can compute from the input and close
  --- over test locals (the same superpower as a `:reply` handler on the HTTP/gRPC mocks).
  --- @param reply any|fun(input): any
  --- @return table self  # for further chaining
  function handle:reply(reply)
    stub.reply = reply
    stub.has_reply = true
    return self
  end
  return handle
end

--- Dispatch one call: record it (with a monotonic `seq`), then answer it. A stub wins; else a
--- `target` (proxy) handles it and the exchange is logged; else — a mock with no match — it raises,
--- because a call you did not predict is a finding, not a default. `d(input)` is sugar for this.
--- @param input any
--- @return any  # the reply
function Double:call(input)
  local seq = #self._journal + 1
  for _, stub in ipairs(self._stubs) do
    if matches(stub.match, input) then
      local reply = stub.reply
      if type(reply) == "function" then reply = reply(input) end
      self._journal[seq] = { seq = seq, input = input, reply = reply, matched = true, source = "stub" }
      return reply
    end
  end
  if self._target then
    local reply = self._target(input)
    self._journal[seq] = { seq = seq, input = input, reply = reply, matched = false, source = "target" }
    return reply
  end
  self._journal[seq] = { seq = seq, input = input, matched = false, source = "unmatched" }
  error(("prova.double: unstubbed call #%d not matched by any stub, and no `target` to proxy to%s")
    :format(seq, self._label and (" ("..self._label..")") or ""), 2)
end

--- Every call the double saw, **in order**, as data: `{ seq, input, reply, matched, source }` where
--- `source` is `"stub"` | `"target"` | `"unmatched"`. `filter` narrows the same way `:on` matches
--- (a subset table or predicate over the recorded `input`) — so you assert on a subset, and on
--- ordering, with the ordinary matchers. Unmatched calls are recorded too.
--- @param filter table|fun(input): boolean|nil
--- @return table[]  # ordered event log
function Double:received(filter)
  local out = {}
  for _, row in ipairs(self._journal) do
    if filter == nil or matches(filter, row.input) then
      out[#out + 1] = shallow(row)
    end
  end
  return out
end

--- Discard the recorded calls (not the stubs) — for reusing one double across phases of a test
--- while asserting on each phase's calls in isolation.
function Double:reset()
  self._journal = {}
end

Double.__index = Double
-- `d(input)` == `d:call(input)`, so a double drops in exactly where a plain function was — the SUT
-- never knows it is talking to a double.
Double.__call = function(self, input) return self:call(input) end

--- Create a double.
--- @param opts { target?: fun(input): any, label?: string }|nil
---   `target` makes it a PROXY: unstubbed calls pass through to it and are logged (stubs still win).
---   Omit `target` for a MOCK: an unstubbed call raises. `label` names it in that error.
--- @return table  # the double — callable, with `:on` / `:call` / `:received` / `:reset`
function M.new(opts)
  opts = opts or {}
  return setmetatable({
    _stubs = {},
    _journal = {},
    _target = opts.target or opts.proxy,
    _label = opts.label,
  }, Double)
end

-- `double(opts)` is sugar for `double.new(opts)`, so `local double = require("prova.double")` then
-- `double{ target = f }` reads naturally.
return setmetatable(M, { __call = function(_, opts) return M.new(opts) end })
