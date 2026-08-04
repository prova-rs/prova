--- The searcher's contract (docs/design/package-system.md § Resolution): where a `require` may
--- resolve from, in what order, and — just as deliberately — where it may NOT. Everything runs in
--- sandbox child packages through the real binary, because resolution is exactly the kind of thing
--- that must be observed from outside: the claim is about what a consumer's `require` sees, not
--- about any internal map.

local scratch = prova.fixture("resolution-scratch", Scope.File, function(ctx)
  return function() return ctx:tempdir() end
end)

local function run(root, env)
  return shell.run(prova.bin, { cwd = root, env = env or {}, merge_stderr = true })
end

prova.test("manifest-declared beats the disk root; intra-package and bundled resolve alongside",
  { covers = "docs/design/package-system.md#resolution-order" }, function(t)
  -- One project exercising three rungs of the ladder at once:
  --   `dup` exists BOTH as a [dependencies] entry (pointing outside the root) and as a package
  --   root directory — the manifest must win, because it is the pinned, authoritative source.
  --   `multi` is a two-file package whose init requires its sibling by canonical name.
  --   `prova.workspace` is bundled — resolvable with no disk package anywhere near it.
  local root = t:use(scratch)()
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]
packages = "pkgs"

[dependencies]
dup = { path = "elsewhere/dup.lua" }
]])
  fs.write(root .. "/elsewhere/dup.lua", 'return { who = "manifest" }\n')
  fs.write(root .. "/pkgs/dup/init.lua", 'return { who = "disk-root" }\n')
  fs.write(root .. "/pkgs/multi/init.lua", 'return { sibling = require("multi.helper").who }\n')
  fs.write(root .. "/pkgs/multi/helper.lua", 'return { who = "sibling" }\n')
  fs.write(root .. "/proofs/order_test.lua", [[
prova.test("the ladder resolves in the declared order", function(t)
  t:expect(require("dup").who, "the manifest is authoritative over the scanned root"):equals("manifest")
  t:expect(require("multi").sibling, "a package requires its own sibling by canonical name"):equals("sibling")
  t:expect(type(require("prova.workspace").create), "bundled modules ride the same searcher"):equals("function")
end)
]])
  t:expect(run(root).stdout):contains("1 passed, 0 failed")
end)

prova.test("only the declared root resolves — not the environment, not the working directory",
  { covers = "docs/design/package-system.md#declared-root-only" }, function(t)
  -- The package is sitting right there in `.prova/packages/` — the old conventional location — and
  -- an env var points at a second copy. Neither may resolve, because the manifest declares no root:
  -- resolution must be readable off `prova.toml` alone, and both of these are invisible from it.
  local root = t:use(scratch)()
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/.prova/packages/ghost/init.lua", 'return { who = "cwd-fallback" }\n')
  fs.write(root .. "/elsewhere/ghost/init.lua", 'return { who = "env-var" }\n')
  fs.write(root .. "/proofs/undeclared_test.lua", [[
prova.test("an undeclared root contributes nothing", function(t)
  local ok, err = pcall(require, "ghost")
  t:expect(ok):equals(false)
  fs.write(prova.root .. "/err.txt", tostring(err))
end)
]])
  local r = run(root, { PROVA_PLUGIN_PATH = root .. "/elsewhere" })
  t:expect(r.stdout):contains("1 passed, 0 failed")
  t:expect(fs.read(root .. "/err.txt"), "the miss really was a miss"):contains('no prova package "ghost"')
end)

prova.test("a miss with no root declared teaches the fix, and the fix is a real key",
  { covers = "docs/design/package-system.md#undeclared-root-teaches" }, function(t)
  -- Having nowhere to look is a different mistake from looking and missing: the message must say
  -- so, and the key it teaches must be one the manifest actually accepts — a teaching message that
  -- names a rejected key sends the reader from one error into another. (That happened: the searcher
  -- once taught `package_root`, which the manifest refuses.)
  local root = t:use(scratch)()
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/miss_test.lua", [[
prova.test("capture the miss", function(t)
  local ok, err = pcall(require, "ghost")
  t:expect(ok):equals(false)
  fs.write(prova.root .. "/err.txt", tostring(err))
end)
]])
  t:expect(run(root).stdout):contains("1 passed, 0 failed")
  local err = fs.read(root .. "/err.txt")
  t:expect(err):contains("no package root declared")
  t:expect(err):contains("add `packages` to [run]")

  -- The taught line, pasted into the manifest, must be accepted — and from then on the message
  -- changes shape: a declared root that misses lists where it looked instead.
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\npackages = ".prova/packages"\n')
  t:expect(run(root).stdout, "the taught key parses"):contains("1 passed, 0 failed")
  local declared = fs.read(root .. "/err.txt")
  t:expect(declared, "a declared root reports where it looked"):contains("no file")
  t:expect(declared):never():contains("no package root declared")
end)
