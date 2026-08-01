--- Black-box spec for run ACTIVITY — the stderr narration that stops a long pause reading as a hang
--- (docs/plans/run-progress-feedback.md, Phase 1).
---
--- The whole feature rests on one guarantee, and it is the guarantee this file exists to hold:
--- **activity can never corrupt a machine format.** A run can sit for tens of seconds inside a cold
--- image pull or a readiness poll, and prova now says what it is doing — but stdout belongs to the
--- reporter (the human tree, `--format json`'s JSONL, TAP), and a single stray byte there breaks
--- every consumer parsing it, agents first. So activity goes to stderr, only to stderr, and these
--- proofs pin that rather than trusting a code review of which stream each `writeln!` picked.
---
--- The activity itself is threshold-gated, so a fast operation is invisible. That makes "did it
--- narrate?" awkward to assert without a genuinely slow operation — which is why the slow cases here
--- use a `shell.run` that sleeps rather than a container pull: same code path (`Kind::Command` is
--- bracketed exactly like `Kind::Pull`), no docker dependency, and it runs in under a second.

local sandbox = prova.fixture("activity-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  -- One proof with a pause long enough to cross the 400ms threshold, and one that is instant.
  fs.write(root .. "/proofs/slow_test.lua", [[
prova.test("a pause worth narrating", function(t)
  local r = shell.run("sleep 1", { check = true })
  t:expect(r.code):equals(0)
end)

prova.test("an instant one", function(t)
  t:expect(1):equals(1)
end)
]])
  return root
end)

--- Run prova in the sandbox with the streams kept APART — the whole point is which is which.
local function run(sb, args, env)
  local merged = { PROVA_PROGRESS = "" }
  for k, v in pairs(env or {}) do merged[k] = v end
  return shell.run(prova.bin .. " " .. (args or ""), { cwd = sb, env = merged })
end

-- ── the guarantee ────────────────────────────────────────────────────────────────────────────

prova.test("activity narrates a real pause, and every word of it lands on stderr", function(t)
  local sb = t:use(sandbox)
  local r = run(sb, "--progress always")

  t:expect(r.stderr, "a 1s pause must be narrated"):contains("running")
  t:expect(r.stderr):contains("sleep 1")
  -- stdout is the reporter's. Not one activity line may appear there.
  t:expect(r.stdout, "activity must not reach stdout"):never():contains("prova: running")
end)

prova.test("--format json stdout stays valid JSONL while activity narrates on stderr", function(t)
  local sb = t:use(sandbox)
  local r = run(sb, "--format json --progress always")

  -- The narration happened...
  t:expect(r.stderr, "activity must still narrate under a machine format"):contains("sleep 1")

  -- ...and every single stdout line is still a parseable JSON object. This is the assertion that
  -- makes the feature safe to leave ON by default: if activity could ever leak to stdout, this is
  -- where it shows up, as a parse failure on a line of prose.
  local lines, seen_finished = 0, false
  for line in r.stdout:gmatch("[^\n]+") do
    lines = lines + 1
    local ok, doc = pcall(json.decode, line)
    t:expect(ok, "stdout line " .. lines .. " is not JSON: " .. line):is_true()
    if ok and type(doc) == "table" and doc.type == "run_finished" then
      seen_finished = true
    end
  end
  t:expect(lines > 0, "stdout produced no JSONL at all"):is_true()
  t:expect(seen_finished, "the run_finished event must survive"):is_true()
end)

prova.test("--format tap stdout stays clean too", function(t)
  local sb = t:use(sandbox)
  local r = run(sb, "--format tap --progress always")

  t:expect(r.stderr):contains("sleep 1")
  -- TAP is line-oriented and unforgiving; a prose line would break a harness reading it.
  t:expect(r.stdout):never():contains("prova: running")
  t:expect(r.stdout):contains("ok ")
end)

-- ── the off switch ───────────────────────────────────────────────────────────────────────────

prova.test("--progress never is completely silent", function(t)
  local sb = t:use(sandbox)
  local r = run(sb, "--progress never")
  t:expect(r.stderr, "nothing may narrate when it is turned off"):never():contains("prova: running")
end)

prova.test("PROVA_PROGRESS=never turns it off without a flag", function(t)
  local sb = t:use(sandbox)
  local r = run(sb, "", { PROVA_PROGRESS = "never" })
  t:expect(r.stderr):never():contains("prova: running")
end)

prova.test("the manifest can turn it off for a whole package", function(t)
  local sb = t:use(sandbox)
  fs.write(sb .. "/prova.toml", '[run]\nproofs = ["proofs"]\nprogress = "never"\n')
  local r = run(sb, "")
  t:expect(r.stderr):never():contains("prova: running")
  -- Put it back — Scope.File means the next test sees this tree.
  fs.write(sb .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
end)

prova.test("a bad progress value is refused, naming what is valid", function(t)
  local sb = t:use(sandbox)
  local r = run(sb, "--progress loud")
  t:expect(r.code):never():equals(0)
  t:expect(r.stderr):contains("auto")
  t:expect(r.stderr):contains("never")
end)

-- ── the threshold ────────────────────────────────────────────────────────────────────────────

prova.test("a fast operation is never narrated", function(t)
  local sb = t:use(sandbox)
  fs.mkdir(sb .. "/fast/proofs")
  fs.write(sb .. "/fast/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(sb .. "/fast/proofs/quick_test.lua", [[
prova.test("instant", function(t)
  local r = shell.run("true", { check = true })
  t:expect(r.code):equals(0)
end)
]])
  local r = shell.run(prova.bin .. " --progress always", {
    cwd = sb .. "/fast",
    env = { PROVA_PROGRESS = "" },
  })

  -- Below the threshold nothing is said at all — not a start line, not a completion. Narrating a
  -- 3ms `true` would train a reader to skim past the lines that actually matter.
  t:expect(r.code, r.stderr):equals(0)
  t:expect(r.stderr, "a trivial command must stay silent"):never():contains("prova: running")
end)
