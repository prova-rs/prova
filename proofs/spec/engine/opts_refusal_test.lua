--- Unknown unit options are refused, not dropped
--- (docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored).
---
--- A dropped option is worse than a rejected one: it reads as configured. `tiemout = "10m"` means
--- "unbounded" and the author believes otherwise; a REMOVED spelling means the behavior it asked
--- for is simply gone, which is how tolerated open specs became hard failures across every
--- consumer that still carried `spec = { … }`.
---
--- Every case drives the SUBJECT: the sandbox package is run by `prova.bin`, so the refusal proven
--- here is this tree's, not the conductor's.

local scaffold = require("scaffold")

local function ran(t, proofs, manifest)
  local proj = scaffold.package(t, { proofs = proofs, manifest = manifest })
  return shell.run(prova.bin, { cwd = proj, merge_stderr = true, timeout = "60s" })
end

prova.test("a typo'd option is refused, naming the key and the spelling meant", {
  covers = "docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored",
  proves = "the silent drop's whole danger is that it reads as configured: `tiemout` means UNBOUNDED, and the suite that believes it is bounded learns otherwise from a hung CI job, not from prova",
}, function(t)
  local r = ran(t, {
    ["typo_test.lua"] = [[
prova.test("bounded, or so it thinks", { tiemout = "10m" }, function(t)
  t:expect(true):is_true()
end)
]],
  })
  t:expect(r.code, "the run refuses"):never():equals(0)
  t:expect(r.stdout, "the offending key is named"):contains("tiemout")
  t:expect(r.stdout, "…with the spelling meant"):contains("timeout")
  t:expect(r.stdout, "…and the declaration's identity, so the fix is one jump"):contains("bounded, or so it thinks")
end)

prova.test("a removed spelling names its successor, not merely itself", {
  covers = "docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored",
  proves = "`spec = { … }` was deleted gone-not-bridged, and every suite still carrying it had tolerated open specs turn into hard failures — 'unknown key' would be true and useless; the author needs the two attributes that replaced it",
}, function(t)
  local r = ran(t, {
    ["removed_test.lua"] = [[
prova.test("carries the retired spelling", { spec = { id = "x", open = true } }, function(t)
  t:expect(true):is_true()
end)
]],
  })
  t:expect(r.code, "the run refuses"):never():equals(0)
  t:expect(r.stdout, "the removal is dated, not merely asserted"):contains("0.18")
  t:expect(r.stdout, "the open-work successor"):contains("promises")
  t:expect(r.stdout, "the obligation-address successor"):contains("covers")
end)

prova.test("the refusal reaches every unit surface — group, flow, step, and suite.config", {
  covers = "docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored",
  proves = "one guarded door is a false sense of a closed house: an option dropped on a GROUP silently un-gates every test under it (a group-level `requires`/`switch` typo is the widest possible silent drop), and `suite.config` is the widest of all",
}, function(t)
  local cases = {
    { name = "group", proof = 'prova.group("g", { swtich = "ut" }, function(g)\n  g:test("t", function(t) t:expect(true):is_true() end)\nend)\n', key = "swtich" },
    { name = "flow", proof = 'prova.flow("f", { requries = { "docker" } }, function(flow)\n  flow:step("s", function(t) t:expect(true):is_true() end)\nend)\n', key = "requries" },
    { name = "step", proof = 'prova.flow("f", function(flow)\n  flow:step("s", { tgas = { "slow" } }, function(t) t:expect(true):is_true() end)\nend)\n', key = "tgas" },
  }
  for _, case in ipairs(cases) do
    local r = ran(t, { ["surface_test.lua"] = case.proof })
    t:expect(r.code, case.name .. " refuses the dropped option"):never():equals(0)
    t:expect(r.stdout, case.name .. " names the key"):contains(case.key)
  end

  -- suite.config is the widest surface: a dropped key here mis-configures every file in the suite.
  local r = ran(t, {
    ["cfg_test.lua"] = 'prova.test("t", function(t) t:expect(true):is_true() end)\n',
    ["suite.lua"] = 'suite.config{ name = "s", reqiures = { "docker" } }\n',
  }, '[run]\nproofs = ["proofs"]\n')
  t:expect(r.code, "suite.config refuses"):never():equals(0)
  t:expect(r.stdout, "…naming the key"):contains("reqiures")
end)

prova.test("every accepted option still parses — the gate refuses typos, not the API", {
  covers = "docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored",
  proves = "a closed set is only safe if it is COMPLETE: a gate that forgets one real spelling turns a working suite red at upgrade, so the accepted set is asserted by a unit that declares all of it at once — the negative control that makes the three refusals above meaningful",
}, function(t)
  local r = ran(t, {
    ["accepted_test.lua"] = [[
local dep = prova.test("a dependency", function(t) t:expect(true):is_true() end)
prova.test("carries every accepted option", {
  timeout = "30s",
  tags = { "slow" },
  depends_on = { dep },
  locks = { prova.writes("opts-gate") },
  serial = true,
  requires = {},
  covers = "SANDBOX-1",
  proves = "the accepted set parses",
  falsified_by = function(t) end,
}, function(t)
  t:expect(true):is_true()
end)
prova.group("switched", { switch = "never-thrown" }, function(g)
  g:test("deselected", function(t) t:expect(true):is_true() end)
end)
]],
  })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the fully-optioned unit ran"):contains("carries every accepted option")
end)
