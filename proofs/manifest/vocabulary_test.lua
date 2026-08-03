--- The package-vocabulary bridge: `plugin` spellings are deprecated, not dead. Every old
--- spelling still WORKS for one release and WARNS once naming its successor — the
--- spec→promises pattern applied to the manifest — and all of them retire together at 1.0.
---
--- The contract (docs/design/manifest.md): `[plugins]`→`[dependencies]`, `plugin_root`→
--- `packages`, `plugin =`→`package =` in [topologies], `[plugin]`→`[package]`; the verbs
--- `prova plugins`/`prova plugin` → `prova packages`/`prova package`. Canonical spellings
--- warn nothing.

local scratch = prova.fixture("vocabulary-scratch", Scope.File, function(ctx)
  return function() return ctx:tempdir() end
end)

local function run(dir, args)
  return shell.run(prova.bin .. (args and (" " .. args) or ""), {
    cwd = dir,
    merge_stderr = true,
  })
end

local function green_pkg(root, manifest)
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", manifest)
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("still runs", function(t) t:expect(1):equals(1) end)\n')
end

prova.test("the old manifest spellings still work, each warning once and naming its successor",
  { covers = "docs/design/manifest.md#deprecated-spellings-teach" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/plugins")
  fs.write(root .. "/plugins/kitchen.lua", [[
local M = {}
function M.orders(ctx) return { svc = { url = "http://127.0.0.1:19999" } } end
return M
]])
  green_pkg(root, [[
[run]
proofs = ["proofs"]
plugin_root = "plugins"

[plugins]
kitchen = "plugins/kitchen.lua"

[topologies]
orders = { plugin = "kitchen", factory = "orders" }
]])

  local r = run(root)
  t:expect(r.code, "deprecated spellings still run green"):equals(0)
  t:expect(r.stdout):contains("still runs")
  t:expect(r.stdout, "[plugins] teaches its successor"):contains("[dependencies]")
  t:expect(r.stdout, "plugin_root teaches its successor"):contains("`packages`")
  t:expect(r.stdout, "the topologies key teaches its successor"):contains("package =")
  t:expect(r.stdout):contains("deprecated")

  -- And the deprecated topology spelling is still addressable by the inhabited verbs.
  local started = run(root, "start orders")
  t:expect(started.code):equals(0)
  run(root, "down orders")
end)

prova.test("the canonical spellings warn nothing",
  { covers = "docs/design/manifest.md#deprecated-spellings-teach" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/packages")
  fs.write(root .. "/packages/kitchen.lua", [[
local M = {}
function M.orders(ctx) return { svc = { url = "http://127.0.0.1:19999" } } end
return M
]])
  green_pkg(root, [[
[run]
proofs = ["proofs"]
packages = "packages"

[dependencies]
kitchen = "packages/kitchen.lua"

[topologies]
orders = { package = "kitchen", factory = "orders" }
]])

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "no deprecation chatter on the new vocabulary"):never():contains("deprecated")
end)

prova.test("a dual-role manifest's [plugin] declaration warns toward [package]",
  { covers = "docs/design/manifest.md#deprecated-spellings-teach" }, function(t)
  -- The kitchen-sink shape: one prova.toml wearing both hats. The old self-declaration spelling
  -- still declares, and teaches.
  local root = t:use(scratch)()
  green_pkg(root, [[
[plugin]
name = "greet"
entry = "greet.lua"

[run]
proofs = ["proofs"]
]])
  fs.write(root .. "/greet.lua", 'return { hello = function() return "hi" end }\n')

  local r = run(root)
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "[plugin] teaches its successor"):contains("[package]")
end)

prova.test("the deprecated verbs still dispatch, warning toward the package spellings",
  { covers = "docs/design/manifest.md#deprecated-spellings-teach" }, function(t)
  local root = t:use(scratch)()
  green_pkg(root, '[run]\nproofs = ["proofs"]\n')

  local listing = run(root, "plugins")
  t:expect(listing.stdout, "old verb warns"):contains("`prova packages`")
  t:expect(listing.stdout):contains("deprecated")

  local canonical = run(root, "packages")
  t:expect(canonical.stdout, "new verb is quiet"):never():contains("deprecated")
end)
