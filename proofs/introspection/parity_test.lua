--- THE PARITY PROOF for `prova.help()` — introspection must be TRUE (docs/plans/autodidact.md M0).
---
--- `help()` is generated from the embedded LuaCATS stubs, which are hand-written; a stub can
--- document a function that was never registered (this happened: `prova.before_each` and seven
--- siblings advertised callables that raised "attempt to call a nil value"). This proof holds the
--- two surfaces to each other, both directions:
---
---   forward:  every dotted FUNCTION entry `help()` returns resolves to a real callable in the
---             live environment — introspection can never again advertise a phantom.
---   reverse:  every function the core globals actually carry appears in `help()` — a registered
---             surface can never again be invisible (this happened too: `prova.workspace`).
---
--- Method entries (`Foo:bar`) and class shapes (`prova.ShellResult`) are not resolvable without an
--- instance, so the forward walk covers dotted names only — which is exactly the set an agent can
--- call from `prova eval` without ceremony.

-- Where a dotted entry's root resolves. Most roots are globals; bundled first-party modules load
-- through the same searcher user plugins use.
local roots = {
  prova = prova,
  shell = shell,
  fs = fs,
  net = net,
  http = http,
  grpc = grpc,
  graphql = graphql,
  yaml = yaml,
  json = json,
  toml = toml,
  csv = csv,
  base64 = base64,
  hash = hash,
  uuid = uuid,
  url = url,
  sqlite = sqlite,
  docker = docker,
  archetect = archetect,
  suite = suite,
  Double = require("prova.double"),
  workspace = require("prova.workspace"),
}

-- `runtime.*` exists only in the `prova.lua` companion; in a test state it is a deliberate
-- error-stub (indexing raises with guidance). Raising IS the registered behavior — treat it as
-- present, don't index it.
local companion_only = { runtime = true }

local function is_function_entry(e)
  return e.signature:sub(1, 1) == "("
end

prova.test("every dotted function help() advertises exists and is callable", function(t)
  local entries = prova.help()
  t:expect(#entries, "a substantial surface"):gt(20)

  local missing = {}
  for _, e in ipairs(entries) do
    local root_name, rest = e.name:match("^([%w_]+)%.([%w_%.]+)$")
    if is_function_entry(e) and root_name and not e.name:find(":") then
      if companion_only[root_name] then
        -- documented, and its guard-rail registration is asserted separately below
      else
        -- Core roots are globals; a PLUGIN's entries (its library/ stub rides the same rail)
        -- resolve through the same require() a proof would use.
        local root = roots[root_name]
        if root == nil then
          local ok, required = pcall(require, root_name)
          root = ok and required or nil
        end
        t:expect(root ~= nil, "help() names root `" .. root_name .. "` — it must resolve"):is_true()
        if root ~= nil then
          local value = root
          for part in rest:gmatch("[^%.]+") do
            local ok, next_value = pcall(function() return value[part] end)
            value = ok and next_value or nil
          end
          local callable = type(value) == "function"
            or (type(value) == "table" and getmetatable(value) and getmetatable(value).__call ~= nil)
          if not callable then missing[#missing + 1] = e.name end
        end
      end
    end
  end
  t:expect(table.concat(missing, ", "), "phantom entries — documented but not registered"):equals("")
end)

prova.test("the phantom hooks stay dead: no xunit hooks, in help() or in the runtime", function(t)
  -- The regression this proof exists for: stubs once documented before_each/after_each/
  -- before_all/after_all (file-level and group-level) that nothing registered. Fixtures hold
  -- setup and teardown together — that is the model, so these must be absent from BOTH surfaces.
  for _, name in ipairs({ "before_each", "after_each", "before_all", "after_all" }) do
    t:expect(prova[name], "prova." .. name .. " must not exist"):is_nil()
    t:expect(#prova.help("prova." .. name), "help() must not advertise prova." .. name):equals(0)
  end
end)

-- Retired spellings: still registered so suites written against them keep running, and deliberately
-- absent from help() so nothing advertises them (their stubs carry `---@deprecated`). This is the one
-- sanctioned gap in reverse parity, and it is ENUMERATED — an *accidental* invisible surface must
-- still fail the proof below. `retired_words_test.lua` proves each one still behaves as its successor.
local retired = {
  ["prova.resource"] = "prova.writes",
  ["prova.shared"] = "prova.reads",
}

prova.test("every function the core globals carry is in help()", function(t)
  local entries = prova.help()
  local documented = {}
  for _, e in ipairs(entries) do documented[e.name] = true end

  -- The reverse direction: a surface an author can touch that help() cannot answer for is the
  -- original agent-ergonomics §0 failure (guess, then probe). Table-valued globals only — userdata
  -- modules enumerate nothing, and their methods are covered by the class stubs.
  local surfaces = {
    prova = prova, shell = shell, fs = fs, net = net,
    json = json, toml = toml, csv = csv,
    base64 = base64, hash = hash, uuid = uuid, url = url,
    workspace = require("prova.workspace"),
  }
  local undocumented = {}
  for surface_name, surface in pairs(surfaces) do
    if type(surface) == "table" then
      for key, value in pairs(surface) do
        local dotted = surface_name .. "." .. key
        if type(value) == "function" and not documented[dotted] and not retired[dotted] then
          undocumented[#undocumented + 1] = dotted
        end
      end
    end
  end
  table.sort(undocumented)
  t:expect(table.concat(undocumented, ", "), "registered but invisible to help()"):equals("")
end)

prova.test("the retired resource words stay callable but unadvertised", function(t)
  -- Both halves of a soft deprecation, in one place: still there (nobody's suite breaks on upgrade),
  -- still silent (nobody LEARNS them from prova's own surface). The exemption above is why this
  -- proof must exist — an unenumerated hole in reverse parity would let a real regression hide here.
  -- The retired words' *scheduling* semantics (writer ⊥ writer, reader ∥ reader) are pinned in
  -- prova-core's `resources` tests, where span overlap is observable.
  -- Exact names, not `prova.help(needle)`: the filter matches substrings, so "prova.resource" also
  -- hits the `prova.ResourceRef` class — which is documentation we WANT, and would mask the check.
  local documented = {}
  for _, e in ipairs(prova.help()) do documented[e.name] = true end

  for old, new in pairs(retired) do
    local old_name = old:match("%.(.+)$")
    local new_name = new:match("%.(.+)$")
    t:expect(type(prova[old_name]), old .. " must stay callable"):equals("function")
    t:expect(type(prova[new_name]), new .. " must exist"):equals("function")
    t:expect(documented[old] == true, "help() must not advertise " .. old):is_false()
    t:expect(documented[new] == true, "help() must answer for " .. new):is_true()
    -- A ref is opaque by design (that is why it can't be typo'd) — what a proof can see from here is
    -- that the retired constructor still yields one a `resources` list accepts.
    t:expect(prova[old_name]("db"), old .. " must still construct a ref"):never():is_nil()
  end
end)

prova.test("help() filters and answers the shapes that once cost probes", function(t)
  -- Pin the original agent-ergonomics round-trips forever.
  for _, name in ipairs({ "shell.run", "shell.spawn", "prova.ShellResult", "Context:tempdir",
                          "workspace.create" }) do
    local hits = prova.help(name)
    local found = false
    for _, e in ipairs(hits) do
      if e.name == name then found = true end
    end
    t:expect(found, "help() must answer for `" .. name .. "`"):is_true()
  end
end)
