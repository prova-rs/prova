--- The published-package contract surface (docs/design/package-system.md § [package]): the `entry`
--- declaration that frees a package from its consumer's alias, and the `[requires] prova` gate that
--- turns version skew into a clear refusal instead of a mysterious runtime failure.

local scratch = prova.fixture("package-contract-scratch", Scope.File, function(ctx)
  return function() return ctx:tempdir() end
end)

local function run(root)
  return shell.run(prova.bin, { cwd = root, merge_stderr = true })
end

prova.test("a declared entry resolves the package under ANY consumer alias",
  { covers = "docs/design/package-system.md#entry-decouples-alias" }, function(t)
  -- The repo's entry file is `rabbitmq.lua`, declared once in its own manifest. The consumer pulls
  -- it as `mq` — a name matching nothing on disk. Filename convention would miss; the declared
  -- entry must resolve. And the package's intra-package require runs on its CANONICAL name
  -- (`rabbitmq.util`), so it is immune to whatever alias the consumer chose.
  local root = t:use(scratch)()
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
mq = "./repo"
]])
  fs.write(root .. "/repo/prova.toml", '[package]\nname = "rabbitmq"\nentry = "rabbitmq.lua"\n')
  fs.write(root .. "/repo/rabbitmq.lua",
    'return { who = "rabbitmq", helper = require("rabbitmq.util").who }\n')
  fs.write(root .. "/repo/util.lua", 'return { who = "util" }\n')
  fs.write(root .. "/proofs/alias_test.lua", [[
prova.test("the alias is the consumer's business only", function(t)
  t:expect(require("mq").who):equals("rabbitmq")
  t:expect(require("mq").helper, "intra-package requires ride the canonical name"):equals("util")
end)
]])
  t:expect(run(root).stdout):contains("1 passed, 0 failed")
end)

prova.test("a package outside its declared prova range refuses to load, naming the mismatch",
  { covers = "docs/design/package-system.md#requires-prova-gates-load" }, function(t)
  -- `<0.1` admits no version this binary will ever report again. The run must die at resolution —
  -- before any proof executes — with the range, the running version, and both ways out.
  local root = t:use(scratch)()
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
old = "./old"
]])
  fs.write(root .. "/old/prova.toml", '[package]\nname = "old"\n\n[requires]\nprova = "<0.1"\n')
  fs.write(root .. "/old/init.lua", 'return {}\n')
  fs.write(root .. "/proofs/never_test.lua",
    'prova.test("must not run", function(t) t:expect(1):equals(2) end)\n')

  local r = run(root)
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains('package "old"')
  t:expect(r.stdout):contains("requires prova <0.1 but this is " .. prova.version)
  t:expect(r.stdout, "the refusal teaches both ways out"):contains("upgrade prova")
  t:expect(r.stdout, "the gate fired before any proof ran"):never():contains("must not run")
end)
