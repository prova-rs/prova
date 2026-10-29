--- `--reuse` (docs/plans/resume.md#phase-2), black-box: input-keyed reuse ACROSS trees.
---
--- The contract, in one line: **a leaf that declares `inputs` carries a prior PASS forward when those
--- inputs (and its declaring file, and prova) are byte-identical — even though the rest of the tree
--- moved — and the record says it was carried on its inputs, never that it ran, nor that the tree
--- was identical.** A leaf that declares nothing always executes; a failure is never carried.
---
--- The sandbox is a git-tracked package: `keyed` declares `inputs = { "src/a.txt" }`, `plain`
--- declares nothing, and `flaky` declares the same input but is red on its first execution. Each
--- counts its executions in a file OUTSIDE the repository, so a carried proof is SEEN not to run.

local sandbox = prova.fixture("reuse-sandbox", Scope.Test, function(ctx)
  local root = ctx:tempdir()
  local pkg = root .. "/pkg"
  local counts = root .. "/counts"
  fs.mkdir(pkg .. "/proofs")
  fs.mkdir(pkg .. "/src")
  fs.mkdir(counts)
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/src/a.txt", "alpha\n")
  fs.write(pkg .. "/src/b.txt", "unrelated\n")
  fs.write(pkg .. "/proofs/widget_test.lua", string.format([[
local function bump(name)
  local path = %q .. "/" .. name
  local n = fs.exists(path) and tonumber(fs.read(path)) or 0
  fs.write(path, tostring(n + 1))
  return n + 1
end

prova.test("keyed", { inputs = { "src/a.txt" } }, function(t)
  bump("keyed")
  t:expect(fs.read(prova.root .. "/src/a.txt")):contains("alpha")
end)
prova.test("plain", function(t) bump("plain"); t:expect(1):equals(1) end)
prova.test("flaky", { inputs = { "src/a.txt" } }, function(t)
  t:expect(bump("flaky") >= 2, "red on its first execution only"):is_true()
end)
]], counts))
  shell.run({ "git", "init", "-q" }, { cwd = pkg, check = true })
  shell.run({ "git", "add", "-A" }, { cwd = pkg, check = true })
  return { pkg = pkg, counts = counts }
end)

--- `prova` in the package, stderr folded in: the one-line account and the refusals are stderr.
local function run(pkg, args)
  return shell.run(prova.bin .. " " .. (args or ""), {
    cwd = pkg,
    env = { PROVA_VAR_DIR = "" },
    merge_stderr = true,
  })
end

local function count(sb, name)
  local path = sb.counts .. "/" .. name
  return fs.exists(path) and tonumber(fs.read(path)) or 0
end

local function record(sb)
  return json.decode(fs.read(sb.pkg .. "/.prova/var/last-run.json"))
end

prova.test("an input-declaring pass is carried across an unrelated edit, and only it", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  local first = run(sb.pkg)
  t:expect(first.stdout, "the flake is red on the first run"):contains("1 failed")
  local first_id = record(sb).run_id

  -- An UNRELATED tracked file moves: the tree is no longer the one the passes were earned over.
  fs.write(sb.pkg .. "/src/b.txt", "edited\n")
  local resume = run(sb.pkg, "--resume")
  t:expect(resume.code, "same-tree resume refuses a moved tree"):equals(2)

  local r = run(sb.pkg, "--reuse")
  t:expect(r.code, "green once the flake passes:\n" .. r.stdout):equals(0)
  t:expect(r.stdout, "names the kind of reuse"):contains("1 reused (inputs unchanged)")
  t:expect(r.stdout, "never claims the tree was the same"):never():contains("same tree")
  t:expect(r.stdout, "one line of account"):contains("--reuse: 1 of 2 input-declared leaves carried forward")
  t:expect(count(sb, "keyed"), "the carried proof did NOT execute again"):equals(1)
  t:expect(count(sb, "plain"), "a leaf declaring nothing always executes"):equals(2)
  t:expect(count(sb, "flaky"), "a failure is never carried — it executed again"):equals(2)

  local rec = record(sb)
  t:expect(rec.executed["widget_test › keyed"], "spelled reused, never passed"):equals("reused")
  t:expect(rec.reused_from["widget_test › keyed"], "naming the run that executed it"):equals(first_id)
  t:expect(rec.reused_inputs["widget_test › keyed"] ~= nil, "and that its INPUTS carried it"):is_true()
  t:expect(rec.reused_inputs["widget_test › flaky"], "an executed leaf is not reused"):equals(nil)
  t:expect(rec.executed["widget_test › flaky"]):equals("passed")
  t:expect(rec.summary.reused_inputs, "counted apart"):equals(1)
  local second_id = rec.run_id

  -- Reuse of a reuse keeps each pass's ORIGIN: the run that executed it.
  local again = run(sb.pkg, "--reuse")
  t:expect(again.stdout):contains("2 reused (inputs unchanged)")
  rec = record(sb)
  t:expect(rec.reused_from["widget_test › keyed"], "still the first run"):equals(first_id)
  t:expect(rec.reused_from["widget_test › flaky"], "the run where it went green"):equals(second_id)

  -- The declared input itself moves: the key changes, so it executes again.
  fs.write(sb.pkg .. "/src/a.txt", "alpha, edited\n")
  local moved = run(sb.pkg, "--reuse")
  t:expect(moved.code, moved.stdout):equals(0)
  t:expect(moved.stdout, "nothing carried over changed inputs"):never():contains("reused")
  t:expect(count(sb, "keyed"), "an edited input executes"):equals(2)
  t:expect(count(sb, "flaky")):equals(3)
end)

prova.test("without --reuse nothing is carried, though every pass is filed", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local plain = run(sb.pkg)
  t:expect(plain.stdout, "a plain run executes everything"):never():contains("reused")
  t:expect(count(sb, "keyed")):equals(2)
  local r = run(sb.pkg, "--reuse")
  t:expect(r.stdout, "the plain runs seeded the store"):contains("2 reused (inputs unchanged)")
  t:expect(count(sb, "keyed")):equals(2)
end)

prova.test("--reuse refuses where carrying would be dishonest", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local banked = run(sb.pkg, "--reuse --update-baseline")
  t:expect(banked.code, "a reused test took no measurement"):equals(2)
  t:expect(banked.stdout):contains("--reuse: --update-baseline")
  local falsify = run(sb.pkg, "--reuse --falsify")
  t:expect(falsify.code, "a falsify pass is a killed mutant"):equals(2)
  t:expect(count(sb, "keyed"), "and nothing ran"):equals(1)
end)

prova.test("a misdeclared `inputs` is refused, never silently ignored", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["."]\n')
  fs.write(dir .. "/bad_test.lua", 'prova.test("x", { inputs = "src/a.txt" }, function(t) end)\n')
  local r = run(dir)
  t:expect(r.code ~= 0, r.stdout):is_true()
  t:expect(r.stdout, "teaches the shape"):contains("inputs lists the package-relative paths/globs")
end)
