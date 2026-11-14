--- `prova.cargo`, black-box on a real workspace (docs/plans/module-proofs.md#phases, phase 2): a
--- change conducts exactly the modules it can reach, and every other module's account is served
--- from the conduct cache.
---
--- The workspace: `a → b → c`. `c` EMBEDS `data/x.txt` (`include_str!`, outside every crate, so only
--- rustc's dep-info knows it); `b`'s TESTS read `data/y.txt` at run time (`[modules] reads`). Each
--- crate has one test and its own `proofs/` adopting its account with `cargo.verify`; `a`'s second
--- test reads an env var only `cargo.configure{ run = { env = … } }` hands the conduct. The adapter
--- prints one line per run: "N module(s) conducted, M served". A fourth member, `d`, has tests
--- and no proofs/: it is no module, and the counts prove it is never conducted.

local function crate(root, name, dep, body, test)
  fs.mkdir(root .. "/" .. name .. "/src")
  fs.mkdir(root .. "/" .. name .. "/proofs")
  fs.write(root .. "/" .. name .. "/Cargo.toml", table.concat({
    "[package]", 'name = "' .. name .. '"', 'version = "0.1.0"', 'edition = "2021"', "",
    "[dependencies]", dep and (dep .. ' = { path = "../' .. dep .. '" }') or "", "",
  }, "\n"))
  fs.write(root .. "/" .. name .. "/src/lib.rs", body .. "\n#[cfg(test)]\nmod tests {\n" .. test .. "\n}\n")
  fs.write(root .. "/" .. name .. "/proofs/" .. name .. "_test.lua", string.format([[
local cargo = require("prova.cargo")
cargo.configure{ profile = "prova", run = { env = { PROVA_CARGO_MARK = "through" } } }
prova.test("%s's cases pass", { locks = { prova.writes("cargo") } }, function(t) cargo.verify(t) end)
]], name))
end

local workspace = prova.fixture("cargo-workspace", Scope.Test, function(ctx)
  local root = ctx:tempdir() .. "/ws"
  fs.mkdir(root .. "/data")
  fs.mkdir(root .. "/.config")
  fs.write(root .. "/data/x.txt", "x1")
  fs.write(root .. "/data/y.txt", "y1")
  fs.write(root .. "/Cargo.toml", '[workspace]\nmembers = ["a", "b", "c", "d"]\nresolver = "2"\n')
  fs.write(root .. "/.config/nextest.toml", '[profile.prova]\njunit = { path = "junit.xml" }\n')
  fs.write(root .. "/.gitignore", "target/\n.prova/\n")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n[modules]\nfrom = ["cargo"]\nreads = { "b" = ["data/y.txt"] }\n')
  crate(root, "c", nil, 'pub const X: &str = include_str!("../../data/x.txt");',
    '#[test] fn embeds() { assert!(super::X.starts_with("x")); }')
  crate(root, "b", "c", "pub fn x() -> &'static str { c::X }",
    '#[test] fn reads() { let y = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../data/y.txt")).unwrap(); assert!(y.starts_with("y")); }')
  crate(root, "a", "b", "pub fn x() -> &'static str { b::x() }", "#[test] fn chains() { assert!(super::x().starts_with(\"x\")); }\n#[test] fn hears_the_conducts_env() { assert_eq!(std::env::var(\"PROVA_CARGO_MARK\").as_deref(), Ok(\"through\")); }")
  -- d is a workspace member with tests and NO proofs/: no module, so never this adapter's to conduct.
  crate(root, "d", nil, "pub fn d() {}", "#[test] fn not_a_module() {}")
  fs.remove_all(root .. "/d/proofs")
  shell.run({ "cargo", "generate-lockfile", "--offline" }, { cwd = root, check = true })
  shell.run({ "git", "init", "-q" }, { cwd = root, check = true })
  shell.run({ "git", "add", "-A" }, { cwd = root, check = true })
  return root
end)

local function run(root, args, env)
  local e = { PROVA_VAR_DIR = "" }
  for k, v in pairs(env or {}) do e[k] = v end
  local r = shell.run(prova.bin .. " " .. (args or ""), {
    cwd = root, env = e, merge_stderr = true, idle_timeout = "600s",
  })
  local conducted, served = r.stdout:match("(%d+) module%(s%) conducted, (%d+) served")
  return r, tonumber(conducted), tonumber(served)
end

local function expect_run(t, root, label, conducted, served, args)
  local r, c, s = run(root, args)
  t:expect(r.code, label .. ":\n" .. r.stdout):equals(0)
  t:expect(string.format("%s conducted, %s served", c, s), label .. ":\n" .. r.stdout)
    :equals(string.format("%d conducted, %d served", conducted, served))
end

prova.test("a change conducts exactly the modules it can reach, and the rest are served", {
  requires = { "cargo", "cargo-nextest", "git" },
  covers = "docs/plans/module-proofs.md#2-the-adapter-answers-one-question",
  timeout = "1800s",
}, function(t)
  local ws = t:use(workspace)
  expect_run(t, ws, "the first run conducts all three", 3, 0)
  expect_run(t, ws, "nothing changed: all three served", 0, 3)

  fs.write(ws .. "/a/src/lib.rs", fs.read(ws .. "/a/src/lib.rs") .. "\n// edited\n")
  expect_run(t, ws, "an edit to a, which nothing depends on: only a", 1, 2)

  fs.write(ws .. "/c/src/lib.rs", fs.read(ws .. "/c/src/lib.rs") .. "\n// edited\n")
  expect_run(t, ws, "an edit to c reaches b and a through their dependencies", 3, 0)

  fs.write(ws .. "/data/x.txt", "x2")
  expect_run(t, ws, "a file c embeds (only dep-info knows it) reaches all three", 3, 0)

  fs.write(ws .. "/data/y.txt", "y2")
  expect_run(t, ws, "a file only b's tests read reaches b alone", 1, 2)

  fs.mkdir(ws .. "/target/scratch")
  fs.write(ws .. "/target/scratch/noise.txt", "noise")
  expect_run(t, ws, "build output under target/ reaches nothing", 0, 3)

  expect_run(t, ws, "--uncached conducts everything (the audit)", 3, 0, "--uncached")
end)

prova.test("a red module is conducted again and never served", {
  requires = { "cargo", "cargo-nextest", "git" },
  covers = "docs/plans/module-proofs.md#3-cache-the-conduct-never-the-proof",
  timeout = "1800s",
}, function(t)
  local ws = t:use(workspace)
  fs.write(ws .. "/data/y.txt", "not y")
  local r, c, s = run(ws)
  t:expect(r.code, "b's test reads y and fails:\n" .. r.stdout):never():equals(0)
  t:expect(string.format("%s/%s", c, s), "all three conducted on a fresh workspace"):equals("3/0")
  local again, c2 = run(ws)
  t:expect(again.code):never():equals(0)
  t:expect(c2, "the red module was not filed, so it conducts again"):gte(1)
end)

--- Herald builds under a CARGO_TARGET_DIR outside the tree, and nextest still writes its junit under
--- the WORKSPACE's target/nextest (its store is workspace-relative): the adapter finds it there
--- (2026-11-05, Substrate's pilot went red on "nextest exited 0 and wrote no junit").
prova.test("a build under a CARGO_TARGET_DIR elsewhere still finds nextest's junit", {
  requires = { "cargo", "cargo-nextest", "git" },
  covers = "docs/plans/module-proofs.md#3-cache-the-conduct-never-the-proof",
  timeout = "1800s",
}, function(t)
  local ws = t:use(workspace)
  local elsewhere = t:tempdir() .. "/build"
  local r, c = run(ws, "", { CARGO_TARGET_DIR = elsewhere })
  t:expect(r.code, "green with the build elsewhere:\n" .. r.stdout):equals(0)
  t:expect(c, "every module conducted"):equals(3)
end)

--- A workspace cargo cannot read offline (Herald gates `--offline`) fails each proof with cargo's own
--- words, never as "no workspace member's directory": a metadata failure leaves no members, and the
--- error must name why (E m/11537: substrate d06baf50 went red on the misleading one, its cause, a
--- dropped `[patch]`, only in the log's second line).
prova.test("a workspace cargo cannot read says cargo's error, never that a module is no member", {
  requires = { "cargo", "cargo-nextest", "git" },
  covers = "docs/plans/module-proofs.md#2-the-adapter-answers-one-question",
  timeout = "600s",
}, function(t)
  local ws = t:use(workspace)
  -- A dependency the lock does not hold yet: `cargo metadata --locked` refuses the tree, while nextest
  -- (which may update the lock) still builds and runs it, as in the gate.
  fs.write(ws .. "/a/Cargo.toml", fs.read(ws .. "/a/Cargo.toml") .. 'd = { path = "../d" }\n')
  local r = run(ws)
  t:expect(r.code, "red:\n" .. r.stdout):never():equals(0)
  t:expect(r.stdout, "names cargo's failure"):contains("cargo metadata failed")
  -- Its first line is cargo's own error, so a classifier keyed on the first line reads it.
  t:expect(r.stdout, "cargo's error line first"):contains("prova.cargo: cargo metadata failed: error:")
  t:expect(r.stdout, "never the misleading member error"):never():contains("is no workspace member's directory")
end)
