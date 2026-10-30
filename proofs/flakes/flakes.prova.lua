--- The flake ledger, black-box: a test that FAILED and PASSED on the same tree and prova is FLAKY —
--- a concurrency defect, never "load", and never green.
---
--- The contract, in one line: **a failure and a pass recorded for one test on one tree digest and
--- one prova binary make it flaky — across runs, within a `--resume`, or by the deputy's own
--- retry — and a flaky run says so in its tally, its record (`verdict: "flaky"`), and the
--- append-only `.prova/var/flakes.jsonl`, and exits non-zero unless `--flakes=warn`.**
---
--- The sandbox is a git-tracked package with one steady proof and one that is red on its first
--- execution only; each counts its executions in a file OUTSIDE the repository, so the second run is
--- over the very same tracked bytes.

local sandbox = prova.fixture("flakes-sandbox", Scope.Test, function(ctx)
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

prova.test("steady", function(t) bump("steady"); t:expect(1):equals(1) end)
prova.test("flaky", function(t)
  t:expect(bump("flaky") >= 2, "red on its first execution only"):is_true()
end)
]], counts))
  shell.run({ "git", "init", "-q" }, { cwd = pkg, check = true })
  shell.run({ "git", "add", "-A" }, { cwd = pkg, check = true })
  return { pkg = pkg, counts = counts, root = root }
end)

--- `prova` in the package, stderr folded in: the flake verdict line is stderr.
local function run(pkg, args)
  return shell.run(prova.bin .. " " .. (args or ""), {
    cwd = pkg,
    env = { PROVA_VAR_DIR = "" },
    merge_stderr = true,
  })
end

local function record(pkg)
  return json.decode(fs.read(pkg .. "/.prova/var/last-run.json"))
end

--- The ledger's lines, decoded — `{}` when there is none.
local function ledger(pkg)
  local path = pkg .. "/.prova/var/flakes.jsonl"
  local out = {}
  if not fs.exists(path) then
    return out
  end
  for line in fs.read(path):gmatch("[^\n]+") do
    out[#out + 1] = json.decode(line)
  end
  return out
end

prova.test("red then green on one tree is FLAKY: the tally, the record, the ledger, and exit 1", {
  requires = { "git" },
  covers = "docs/design/flakes.md#the-rule",
}, function(t)
  local sb = t:use(sandbox)
  local first = run(sb.pkg)
  t:expect(first.code, first.stdout):equals(1)
  local first_id = record(sb.pkg).run_id

  local r = run(sb.pkg)
  t:expect(r.code, "a flake is a defect — it fails by default:\n" .. r.stdout):equals(1)
  t:expect(r.stdout, "the tally shouts it, apart from passed"):contains("1 passed, 0 failed, 1 FLAKY")
  t:expect(r.stdout, "the recap names it"):contains("FLAKY  widget_test › flaky")
  t:expect(r.stdout, "with the failure it passed after"):contains("red on its first execution only")
  t:expect(r.stdout, "and both runs"):contains("failed in " .. first_id)
  local rec = record(sb.pkg)
  t:expect(rec.verdict, "never green"):equals("flaky")
  t:expect(rec.executed["widget_test › flaky"]):equals("flaky")
  t:expect(rec.executed["widget_test › steady"]):equals("passed")
  t:expect(rec.summary.flaky):equals(1)

  local lines = ledger(sb.pkg)
  t:expect(#lines, "one ledger line per flake event"):equals(1)
  local e = lines[1]
  t:expect(e.kind):equals("flake")
  t:expect(e.schema):equals(1)
  t:expect(e.test):equals("widget_test › flaky")
  t:expect(e.source, "found across runs"):equals("rerun")
  t:expect(e.failed_run):equals(first_id)
  t:expect(e.passed_run):equals(rec.run_id)
  t:expect(e.tree, "keyed on the tree"):equals(rec.tree)
  t:expect(e.binary, "and the prova"):equals(rec.binary)
  t:expect(e.first_failure):contains("red on its first execution only")
  t:expect(e.deputed, "a prova leaf, not a deputed case"):equals(nil)

  -- Still the same tree: the failure is still recorded on it, so it is still flaky — and the ledger
  -- APPENDS (never truncated), keeping the first event.
  local third = run(sb.pkg)
  t:expect(third.code, third.stdout):equals(1)
  lines = ledger(sb.pkg)
  t:expect(#lines, "append-only"):equals(2)
  t:expect(lines[2].failed_run, "naming the run that FAILED, not the flaky one"):equals(first_id)
end)

prova.test("--flakes=warn reports a flake without failing — and the verdict is still flaky", {
  requires = { "git" },
  covers = "docs/design/flakes.md#exit",
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local r = run(sb.pkg, "--flakes=warn")
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("1 FLAKY")
  t:expect(r.stdout):contains("--flakes=warn")
  t:expect(record(sb.pkg).verdict, "warn changes the exit, never the verdict"):equals("flaky")
  local bad = run(sb.pkg, "--flakes=maybe")
  t:expect(bad.code, "an unknown mode is refused"):equals(2)
end)

prova.test("a pass on a DIFFERENT tree is no flake: the code changed, so did the question", {
  requires = { "git" },
  covers = "docs/design/flakes.md#the-rule",
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local file = sb.pkg .. "/proofs/widget_test.lua"
  fs.write(file, fs.read(file) .. "\n-- an edit\n")
  local r = run(sb.pkg)
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):never():contains("FLAKY")
  local rec = record(sb.pkg)
  t:expect(rec.verdict):equals("green")
  t:expect(rec.executed["widget_test › flaky"]):equals("passed")
  t:expect(#ledger(sb.pkg), "nothing in the ledger"):equals(0)
end)

--- A package whose one proof adopts a junit account: a nextest-style retry (`<flakyFailure>` and no
--- `<failure>`) for one case. No VCS, so no history — the deputy's own mark is enough.
prova.test("a deputed case nextest retried is FLAKY, never passed", {
  covers = "docs/design/flakes.md#deputed-cases",
}, function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.mkdir(dir .. "/proofs")
  fs.write(dir .. "/junit.xml", [[<testsuites><testsuite name="cos-daemon::uds">
  <testcase name="tests::steady" classname="cos-daemon::uds"/>
  <testcase name="tests::bind" classname="cos-daemon::uds"><flakyFailure message="Address already in use"/></testcase>
</testsuite></testsuites>]])
  fs.write(dir .. "/proofs/adopt_test.lua", [[
prova.test("the account holds", function(t) junit.verify(t, { results = prova.root .. "/junit.xml" }) end)
]])
  local r = run(dir)
  t:expect(r.code, "flaky, so not green:\n" .. r.stdout):equals(1)
  t:expect(r.stdout, "the adopting proof itself passed — no case was red"):contains("1 passed, 0 failed, 1 FLAKY")
  t:expect(r.stdout, "junit.verify names the flaky case"):contains("cos-daemon::uds#tests::bind")
  t:expect(r.stdout, "the recap says how to reproduce it"):contains("prova stress -E 'binary_id(=cos-daemon::uds) & test(=tests::bind)'")
  local rec = record(dir)
  local outcome = {}
  for _, d in ipairs(rec.deputed) do
    outcome[d.name] = d.outcome
  end
  t:expect(outcome["tests::bind"], "the deputed row keeps it flaky"):equals("flaky")
  t:expect(outcome["tests::steady"]):equals("passed")
  t:expect(rec.verdict):equals("flaky")
  local e = ledger(dir)[1]
  t:expect(e.source):equals("junit-retry")
  t:expect(e.deputed.suite):equals("cos-daemon::uds")
  t:expect(e.deputed.name):equals("tests::bind")
  t:expect(e.first_failure):equals("Address already in use")
end)

prova.test("a deputed case red in one run and green in the next on one tree is FLAKY", {
  requires = { "git" },
  covers = "docs/design/flakes.md#deputed-cases",
}, function(t)
  local root = t:tempdir()
  local pkg = root .. "/pkg"
  fs.mkdir(pkg .. "/proofs")
  -- The accounts live OUTSIDE the repository, so the tracked tree is identical across both runs.
  fs.write(root .. "/red.xml", [[<testsuites><testsuite name="s"><testcase name="t" classname="s"><failure message="lost the race"/></testcase></testsuite></testsuites>]])
  fs.write(root .. "/green.xml", [[<testsuites><testsuite name="s"><testcase name="t" classname="s"/></testsuite></testsuites>]])
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/adopt_test.lua", string.format([[
prova.test("adopt", function(t)
  local n = %q .. "/n"
  local first = not fs.exists(n)
  fs.write(n, "1")
  junit.verify(t, { results = first and %q or %q })
end)
]], root, root .. "/red.xml", root .. "/green.xml"))
  shell.run({ "git", "init", "-q" }, { cwd = pkg, check = true })
  shell.run({ "git", "add", "-A" }, { cwd = pkg, check = true })

  t:expect(run(pkg).code):equals(1)
  local r = run(pkg)
  t:expect(r.code, r.stdout):equals(1)
  local rec = record(pkg)
  t:expect(rec.deputed[1].outcome, "re-marked flaky by the record diff"):equals("flaky")
  local by = {}
  for _, e in ipairs(rec.flakes) do
    by[e.test] = e
  end
  t:expect(by["s#t"] ~= nil, "the deputed case is a flake"):is_true()
  t:expect(by["s#t"].source):equals("rerun")
  t:expect(by["s#t"].first_failure):equals("lost the race")
  t:expect(by["adopt_test › adopt"] ~= nil, "and so is the proof that adopted it"):is_true()
end)
