--- The advertisement seam (docs/design/package-system.md § Topologies): a package publishes its
--- topologies in `[[package.topologies]]`, a consumer registers one by advertised NAME — never by
--- reaching into the package's internals — and the advertisement's `requires` travel with the
--- topology, gating `up` before anything is provisioned. Resourceless throughout, like
--- verbs_test.lua: the machinery under proof is resolution and gating, not Docker.

local scratch = prova.fixture("advertisement-scratch", Scope.File, function(ctx)
  return function() return ctx:tempdir() end
end)

local function run(dir, args, env)
  return shell.run(prova.bin .. " " .. args, { cwd = dir, env = env or {}, merge_stderr = true })
end

--- A package advertising one topology that requires a capability no machine has, consumed by name.
--- The factory writes a marker if it ever runs, so "gated BEFORE provisioning" is observable.
local function advertised(root)
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
parallels = "./pkg"

[topologies]
vm    = { package = "parallels", topology = "linux-vm" }
local = { package = "parallels", factory = "topologies.plain", requires = ["definitely_absent_capability"] }
]])
  -- `[=[ … ]=]` deliberately: the manifest's own `[[package.topologies]]` would close a plain
  -- `[[ … ]]` long string at its first `]]`.
  fs.write(root .. "/pkg/prova.toml", [=[
[package]
name = "parallels"
entry = "init.lua"

[[package.topologies]]
name     = "linux-vm"
factory  = "topologies.linux_vm"
requires = ["definitely_absent_capability"]
]=])
  fs.write(root .. "/pkg/init.lua", [[
local M = { topologies = {} }
local function factory(ctx)
  local marker = os.getenv("PROVA_PROOF_MARKER")
  if marker then fs.write(marker, "factory-ran") end
  return { vm = { url = "http://127.0.0.1:19998" } }
end
M.topologies.linux_vm = factory
M.topologies.plain = factory
return M
]])
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the suite runs", function(t) t:expect(1):equals(1) end)\n')
end

prova.test("an advertised topology registers by name and lists under `up`",
  { covers = "docs/design/package-system.md#topology-advertisement-resolves" }, function(t)
  local root = t:use(scratch)()
  advertised(root)
  local r = run(root, "up")
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "registered via the advertisement, not a factory path"):contains("vm")
end)

prova.test("an advertised name the package does not publish is refused, naming what IS available",
  { covers = "docs/design/package-system.md#topology-advertisement-resolves" }, function(t)
  local root = t:use(scratch)()
  advertised(root)
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
parallels = "./pkg"

[topologies]
vm = { package = "parallels", topology = "windows-vm" }
]])
  local r = run(root, "up")
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains('advertises no topology "windows-vm"')
  t:expect(r.stdout, "the refusal names the real contract"):contains("linux-vm")
end)

prova.test("the advertisement's requires gate `up` before the factory ever runs",
  { covers = "docs/design/package-system.md#up-gates-on-requires" }, function(t)
  -- The consumer's registration never mentions the requirement — it travels with the topology,
  -- from the package's own advertisement. And the marker proves the stop happened up front: an
  -- unmet environment must be a clear early refusal, not a failure deep inside a factory.
  local root = t:use(scratch)()
  advertised(root)
  local marker = root .. "/factory-marker"
  local r = run(root, "up vm", { PROVA_PROOF_MARKER = marker })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains('cannot stand up topology "vm"')
  t:expect(r.stdout):contains('requires "definitely_absent_capability"')
  t:expect(fs.exists(marker), "refused before provisioning"):equals(false)
end)

prova.test("a registration's own requires gate the same way — merged on top of the advertisement",
  { covers = "docs/design/package-system.md#up-gates-on-requires" }, function(t)
  -- `local` is registered through the direct factory path with a registration-level requires: the
  -- local addition must gate exactly like an advertised one.
  local root = t:use(scratch)()
  advertised(root)
  local marker = root .. "/factory-marker"
  local r = run(root, "up local", { PROVA_PROOF_MARKER = marker })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains('cannot stand up topology "local"')
  t:expect(r.stdout):contains('requires "definitely_absent_capability"')
  t:expect(fs.exists(marker)):equals(false)
end)
