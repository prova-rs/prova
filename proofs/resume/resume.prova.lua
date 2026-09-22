--- `--resume` (docs/plans/resume.md#phase-1a), black-box: a red run costs its failures, not the suite.
---
--- The contract, in one line: **a pass is carried forward only over the identical tree, the same
--- prova and the same lane, and the record says it was carried, never that it ran.** Every other
--- case REFUSES, exit 2, naming why: a resume that quietly ran everything would hold its caller for
--- the whole suite while it believed it was resuming.
---
--- The sandbox is a git-tracked package with two steady proofs and one flake. Each counts its own
--- executions in a file OUTSIDE the repository, so a second run can go green over the same tracked
--- bytes, and a reused proof is SEEN not to execute (its counter stays put).

local sandbox = prova.fixture("resume-sandbox", Scope.Test, function(ctx)
  local root = ctx:tempdir()
  local pkg = root .. "/pkg"
  local counts = root .. "/counts"
  fs.mkdir(pkg .. "/proofs")
  fs.mkdir(counts)
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/widget_test.lua", string.format([[
local function bump(name)
  local path = %q .. "/" .. name
  local n = fs.exists(path) and tonumber(fs.read(path)) or 0
  fs.write(path, tostring(n + 1))
  return n + 1
end

prova.test("steady one", function(t) bump("one"); t:expect(1):equals(1) end)
prova.test("steady two", function(t) bump("two"); t:expect(2):equals(2) end)
prova.test("flaky", function(t)
  -- What the run tells a deputy about itself (docs/plans/resume.md#phase-1b), as it saw it.
  fs.write(%q .. "/facts", (prova.run_id or "nil") .. " " .. (prova.resume and prova.resume.from or "nil"))
  t:expect(bump("flaky") >= 2, "red on its first execution only"):is_true()
end)
]], counts, counts))
  shell.run({ "git", "init", "-q" }, { cwd = pkg, check = true })
  shell.run({ "git", "add", "-A" }, { cwd = pkg, check = true })
  return { pkg = pkg, counts = counts }
end)

--- `prova` in the package, stderr folded in: the refusals are stderr, and they are the assertions.
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

prova.test("a resume executes only what did not pass, and carries the rest forward as reused", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  local first = run(sb.pkg)
  t:expect(first.stdout, "the flake is red on the first run"):contains("1 failed")
  local first_id = record(sb).run_id
  t:expect(first_id ~= nil and first_id ~= "", "the record names its run"):is_true()

  local r = run(sb.pkg, "--resume")
  t:expect(r.code, "green once the flake passes:\n" .. r.stdout):equals(0)
  t:expect(r.stdout, "says how much it carried"):contains("2 reused (same tree)")
  t:expect(r.stdout, "and how much it executed"):contains("1 passed")
  t:expect(count(sb, "one"), "a reused proof did NOT execute again"):equals(1)
  t:expect(count(sb, "flaky"), "the red one did"):equals(2)

  local rec = record(sb)
  t:expect(rec.executed["widget_test › steady one"], "spelled reused, never passed"):equals("reused")
  t:expect(rec.executed["widget_test › flaky"], "the executed one is passed"):equals("passed")
  t:expect(rec.reused_from["widget_test › steady one"], "naming the run that executed it"):equals(first_id)
  t:expect(rec.summary.reused, "and counted apart from passed"):equals(2)
end)

prova.test("a conduct sees its own run id, and — only when resuming — the run it resumes", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local first_id = record(sb).run_id
  t:expect(fs.read(sb.counts .. "/facts"), "a plain run: its id, and no resume"):equals(first_id .. " nil")
  run(sb.pkg, "--resume")
  local second_id = record(sb).run_id
  t:expect(second_id ~= first_id, "every run is its own"):is_true()
  t:expect(fs.read(sb.counts .. "/facts"), "a resumed run names the run it resumes")
    :equals(second_id .. " " .. first_id)
end)

prova.test("a resume of a resume keeps the ORIGIN of every carried pass", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local first_id = record(sb).run_id
  run(sb.pkg, "--resume")
  local second_id = record(sb).run_id

  local r = run(sb.pkg, "--resume")
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "everything carried, nothing executed"):contains("3 reused (same tree)")
  local rec = record(sb)
  t:expect(rec.reused_from["widget_test › steady one"], "passed in the first run"):equals(first_id)
  t:expect(rec.reused_from["widget_test › flaky"], "passed in the second"):equals(second_id)
  t:expect(count(sb, "one"), "and never executed again"):equals(1)
end)

prova.test("an edited tracked file REFUSES the resume, naming why, and executes nothing", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local file = sb.pkg .. "/proofs/widget_test.lua"
  fs.write(file, fs.read(file) .. "\n-- an edit\n")

  local r = run(sb.pkg, "--resume")
  t:expect(r.code, "a refusal, not a run"):equals(2)
  t:expect(r.stdout, "names the cause"):contains("tracked tree changed")
  t:expect(r.stdout, "and teaches the alternatives"):contains("--last-failed")
  t:expect(count(sb, "one"), "nothing executed"):equals(1)
end)

prova.test("a narrowed resume, or one with nothing journaled, REFUSES", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  local none = run(sb.pkg, "--resume")
  t:expect(none.code, "nothing to resume from"):equals(2)
  t:expect(none.stdout):contains("no earlier run of this lane")
  t:expect(count(sb, "one"), "and nothing ran"):equals(0)

  run(sb.pkg)
  local narrowed = run(sb.pkg, "-k steady --resume")
  t:expect(narrowed.code, "a resume carries a whole lane"):equals(2)
  t:expect(narrowed.stdout):contains("narrow")
end)
