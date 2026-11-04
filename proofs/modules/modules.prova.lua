--- Module proofs, the engine half (docs/plans/module-proofs.md), black-box through the real binary.
---
--- A proof belongs to the module its `proofs/` sits in (`t.module`), `[modules]` reaches Lua as
--- `prova.modules`, and the conduct cache keeps a filed account across runs: served to a later run
--- under the same key and naming the run that filed it, never served under `--uncached`, and never
--- filed by a run whose conducts are not the tree's (`-u`). The adapter (`prova.cargo`) is built on
--- exactly these; this file proves the engine holds them for any adapter.

local COVER = "docs/plans/module-proofs.md#1-a-proof-belongs-to-its-module"

local sandbox = prova.fixture("modules-sandbox", Scope.Test, function(ctx)
  local root = ctx:tempdir()
  local pkg = root .. "/pkg"
  fs.mkdir(pkg .. "/proofs")
  fs.mkdir(pkg .. "/crates/scm/proofs")
  fs.mkdir(pkg .. "/crates/scm/src")
  fs.write(pkg .. "/crates/scm/src/lib.rs", "pub fn x() {}\n")
  fs.write(pkg .. "/prova.toml", table.concat({
    "[run]",
    'proofs = ["proofs"]',
    "[modules]",
    'from = ["cargo", "swiftpm"]',
    'reads = { "scm" = ["vault/**"] }',
    "",
  }, "\n"))
  fs.write(pkg .. "/proofs/root_test.lua", [[
prova.test("a root proof belongs to no module", function(t)
  t:expect(t.module == nil, "root proofs/: no module"):is_true()
end)
]])
  fs.write(pkg .. "/crates/scm/proofs/scm_test.lua", [[
prova.test("a module proof names its module", function(t)
  t:expect(t.module.dir):equals("crates/scm")
end)
prova.test("[modules] reaches Lua", function(t)
  t:expect(prova.modules.from[1]):equals("cargo")
  t:expect(prova.modules.from[2]):equals("swiftpm")
  t:expect(prova.modules.reads.scm[1]):equals("vault/**")
end)
prova.test("the conduct cache keeps an account", function(t)
  local key = prova.conduct_cache.key({ "cargo", "scm" }, { "crates/scm/src/lib.rs" })
  local filed = prova.conduct_cache.lookup(key)
  local mode = os.getenv("CACHE_MODE")
  if mode == "file" then
    local artifact = prova.root .. "/junit.xml"
    fs.write(artifact, "<testsuites/>")
    prova.conduct_cache.file(key, { junit = artifact })
    print("filed-by:" .. prova.run_id)
  elseif mode == "serve" then
    t:expect(filed ~= nil, "a later run is served the filed account"):is_true()
    t:expect(fs.read(filed.files.junit)):equals("<testsuites/>")
    print("served-from:" .. filed.run_id)
  elseif mode == "uncached" then
    t:expect(filed == nil, "--uncached serves nothing"):is_true()
  elseif mode == "refuse" then
    local ok, err = pcall(prova.conduct_cache.file, key, { junit = prova.root .. "/junit.xml" })
    t:expect(ok, "a -u run files nothing"):is_false()
    t:expect(tostring(err)):contains("-u rewrites expectations")
  end
end)
]])
  -- Filing names the run that executed the conduct, and only a journaled run has an id: a tree.
  shell.run({ "git", "init", "-q" }, { cwd = pkg, check = true })
  shell.run({ "git", "add", "-A" }, { cwd = pkg, check = true })
  return { pkg = pkg }
end)

local function run(pkg, mode, args)
  return shell.run(prova.bin .. " " .. (args or ""), {
    cwd = pkg,
    env = { PROVA_VAR_DIR = "", CACHE_MODE = mode or "" },
    merge_stderr = true,
  })
end

prova.test("a proof belongs to the module its proofs/ sits in, and [modules] reaches Lua", {
  covers = COVER,
}, function(t)
  local sb = t:use(sandbox)
  local r = run(sb.pkg)
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("4 passed")
end)

prova.test("a module proof knows its module when the run names its path", {
  covers = COVER,
}, function(t)
  local sb = t:use(sandbox)
  local r = run(sb.pkg, nil, "./crates/scm/proofs")
  t:expect(r.code, "an explicit-path run, relative as a person types it:\n" .. r.stdout):equals(0)
  t:expect(r.stdout):contains("3 passed")
end)

prova.test("the conduct cache serves a filed account to a later run, naming the run that filed it", {
  requires = { "git" },
  covers = "docs/plans/module-proofs.md#3-cache-the-conduct-never-the-proof",
}, function(t)
  local sb = t:use(sandbox)
  local filed = run(sb.pkg, "file")
  t:expect(filed.code, filed.stdout):equals(0)
  local by = filed.stdout:match("filed%-by:(%S+)")
  t:expect(by ~= nil, "the filing run printed its id:\n" .. filed.stdout):is_true()

  local served = run(sb.pkg, "serve")
  t:expect(served.code, served.stdout):equals(0)
  t:expect(served.stdout:match("served%-from:(%S+)"), "served from the run that filed it"):equals(by)

  local uncached = run(sb.pkg, "uncached", "--uncached")
  t:expect(uncached.code, "--uncached is served nothing:\n" .. uncached.stdout):equals(0)

  -- An edit inside the key's closure is another conduct: nothing is served under the new key.
  fs.write(sb.pkg .. "/crates/scm/src/lib.rs", "pub fn y() {}\n")
  local moved = run(sb.pkg, "uncached")
  t:expect(moved.code, "an edited closure is another key:\n" .. moved.stdout):equals(0)
end)

prova.test("a run whose conducts are not the tree's files nothing, and says why", {
  covers = "docs/plans/module-proofs.md#3-cache-the-conduct-never-the-proof",
}, function(t)
  local sb = t:use(sandbox)
  local r = run(sb.pkg, "refuse", "-u")
  t:expect(r.code, r.stdout):equals(0)
end)

prova.test("a dependency names its entry file with entry =, and the retired module = is refused", {
  covers = "docs/plans/module-proofs.md#decisions-user-2026-11-04",
}, function(t)
  local root = t:tempdir()
  local pkg, dep = root .. "/pkg", root .. "/dep"
  fs.mkdir(pkg .. "/proofs")
  fs.mkdir(dep .. "/src")
  fs.write(dep .. "/src/greet.lua", 'return { hi = function() return "hi" end }\n')
  fs.write(pkg .. "/proofs/dep_test.lua",
    'prova.test("entry resolves", function(t) t:expect(require("greet").hi()):equals("hi") end)\n')
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n[dependencies]\ngreet = { path = "../dep", entry = "src/greet.lua" }\n')
  local ok = run(pkg)
  t:expect(ok.code, ok.stdout):equals(0)
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n[dependencies]\ngreet = { path = "../dep", module = "src/greet.lua" }\n')
  local old = run(pkg)
  t:expect(old.code, "module = is no longer a key:\n" .. old.stdout):never():equals(0)
  t:expect(old.stdout):contains("module")
end)
