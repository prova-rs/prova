--- A nested run does not write to the job's reporting surfaces — and the depth marker that tells it so.
---
--- `$GITHUB_STEP_SUMMARY` names ONE file for the whole job, and `::error` annotations attach to the
--- job. Both are singular, job-scoped resources; the environment naming them is inherited by every
--- descendant process. That combination is the bug: prova's suites drive prova (see
--- `binary_identity_test.lua` for why that is the model), so under Actions every inner run used to
--- append its own table to the shared summary and annotate its own failures.
---
--- What that produces is worse than noise. The inner runs a suite drives are frequently RED ON
--- PURPOSE — `crates/prova-cli/selftest/fixtures/mixed.lua` is one pass, one fail, one skip
--- precisely so the outer test can assert on a failing tally. Hoisting those into the job summary
--- reports expected outcomes as job failures, repeated once per test that drives the fixture, while
--- the real failure is buried among them. A green run can render red. The summary stops being
--- evidence, which is the only thing it was for.
---
--- The fix is not to teach the reporter about fixtures; it is to notice that `GITHUB_ACTIONS=true`
--- was only ever a proxy for "I am the run this job is watching", and that the proxy breaks under
--- nesting. `PROVA_RUN_DEPTH` carries the real answer: every process prova spawns is stamped with
--- its depth, so `github = "auto"` can ask the question it actually means. An inner run still
--- reports in full — through its exit code and its own stdout, to the test that spawned it. That is
--- the only reader that should ever have cared.
---
--- Note what is NOT gated: `--gha on`. Nesting is not evidence against someone explicitly asking
--- this run to annotate.

--- The marker's name is the contract — it rides on every spawned process, so a rename is a
--- user-visible change and this literal is what makes that break loudly.
local DEPTH = "PROVA_RUN_DEPTH"

--- A one-test package whose single proof FAILS — the shape a suite drives on purpose. Each call
--- gets its own directory; all of them go with the file scope.
local red_package = prova.fixture("nested-reporting-sandbox", Scope.File, function(ctx)
  local nth = 0
  return function(body)
    -- Named per call, so each sandbox is its own directory AND says so on disk.
    nth = nth + 1
    local dir = ctx:tempdir(tostring(nth))
    fs.write(dir .. "/prova.toml", '[package]\nname = "red"\n\n[run]\nproofs = ["."]\n')
    fs.write(dir .. "/red.prova.lua",
      body or 'prova.test("red", function(t) t:expect(1):equals(2) end)\n')
    return dir
  end
end)

--- A fresh, empty step-summary file plus the Actions environment naming it. `env` EXTENDS the
--- inherited environment, so this stands up a believable Actions context on a developer laptop and
--- overrides the real one on a CI runner — the same proof either way.
--- `PROVA_GHA = auto` is not redundant: it neutralizes an inherited setting, so these assertions
--- describe prova rather than whatever the surrounding CI step configured.
local function actions_env(summary)
  fs.write(summary, "")
  return {
    GITHUB_ACTIONS = "true",
    GITHUB_WORKSPACE = prova.root,
    GITHUB_STEP_SUMMARY = summary,
    PROVA_GHA = "auto",
  }
end

prova.test("prova stamps the nesting depth on every process it spawns", function(t)
  -- Observed through prova itself rather than a `printenv`, which does not exist on Windows. The
  -- inner suite asserts on its OWN environment: if the stamp is missing, `os.getenv` is nil and the
  -- inner run exits non-zero. `>= 1`, not `== 1`, because this proof must not care how deep the
  -- outer run already is.
  local dir = t:use(red_package)([[
prova.test("the run carries its depth", function(t)
  local depth = tonumber(os.getenv("PROVA_RUN_DEPTH"))
  t:expect(depth, "a spawned prova must be told its nesting depth"):is_truthy()
  t:expect(depth >= 1, "and a run spawned from inside a suite is nested"):is_true()
end)
]])
  local r = shell.run(prova.bin .. " " .. dir, { merge_stderr = true })
  t:expect(r.code, "the inner assertions on the marker must pass: " .. r.stdout):equals(0)
end)

prova.test("a nested run leaves the job's step summary alone", function(t)
  local summary = fs.tempdir() .. "/step_summary.md"
  local env = actions_env(summary)

  -- Driven from inside a suite, so the child is nested BY CONSTRUCTION — nothing is set by hand
  -- here; the runtime stamps the depth because prova is what is doing the spawning.
  local r = shell.run(prova.bin .. " " .. t:use(red_package)(), { env = env })
  t:expect(r.code, "the inner run still reports its failure through its exit code"):equals(1)
  t:expect(r.stdout, "and still prints its own console report"):contains("1 failed")

  t:expect(fs.read(summary), "but must not touch the job-scoped summary"):equals("")
  t:expect(r.stdout:find("::error", 1, true), "nor annotate the job"):is_falsy()
end)

prova.test("the top-level run is the one that writes — depth is what distinguishes it", function(t)
  local summary = fs.tempdir() .. "/step_summary.md"
  local env = actions_env(summary)
  -- The negative control for the test above, and the reason it means anything: same command, same
  -- Actions environment, the ONLY difference being the depth the child reads. Overridden to 0 to
  -- stand in for the job's own run. Without this, "the summary was empty" is equally well explained
  -- by the sink being broken outright.
  env[DEPTH] = "0"

  local r = shell.run(prova.bin .. " " .. t:use(red_package)(), { env = env })
  t:expect(r.code):equals(1)
  t:expect(fs.read(summary), "a top-level run under Actions writes the table"):contains("prova —")
  t:expect(fs.read(summary), "naming the failing test"):contains("red")
  t:expect(r.stdout, "and annotates the job"):contains("::error")
end)

prova.test("--gha on still annotates from inside a nested run", function(t)
  local summary = fs.tempdir() .. "/step_summary.md"
  local env = actions_env(summary)

  -- The escape hatch: nesting downgrades `auto`'s INFERENCE, never an explicit request.
  local r = shell.run(prova.bin .. " --gha on " .. t:use(red_package)(), { env = env })
  t:expect(r.code):equals(1)
  t:expect(r.stdout, "an explicit --gha on is honored at any depth"):contains("::error")
  t:expect(fs.read(summary), "step summary included"):contains("prova —")
end)

prova.test("PROVA_GHA=off suppresses the sink for a run that is otherwise top-level", function(t)
  -- The depth marker answers "was I spawned by prova?", which is the only nesting prova can see.
  -- A NON-prova harness driving the binary — prova's own ~24 Rust integration tests do, dozens of
  -- times per run — produces children that are top-level as far as they can tell, and no stamp can
  -- reach them: the harness is not prova. PROVA_GHA=off is the seam for that case, and .github/
  -- workflows/build.yml sets it on the `cargo test` step for exactly this reason. Proven here
  -- because a workflow depending on an unproven behavior is how the summary silently refills.
  local summary = fs.tempdir() .. "/step_summary.md"
  local env = actions_env(summary)
  env[DEPTH] = "0"     -- top-level by the depth rule…
  env.PROVA_GHA = "off" -- …and still silent, because the env says so.

  local r = shell.run(prova.bin .. " " .. t:use(red_package)(), { env = env })
  t:expect(r.code):equals(1)
  t:expect(fs.read(summary), "the job summary stays the harness's business"):equals("")
  t:expect(r.stdout:find("::error", 1, true)):is_falsy()
end)

prova.test("outside Actions, nesting changes nothing", function(t)
  -- Guards against the marker leaking into non-CI behavior: with no GITHUB_ACTIONS in play, a
  -- nested run is exactly as quiet as it has always been.
  local r = shell.run(prova.bin .. " " .. t:use(red_package)(), { merge_stderr = true })
  t:expect(r.code):equals(1)
  t:expect(r.stdout:find("::error", 1, true)):is_falsy()
  t:expect(r.stdout):contains("1 failed")
end)
