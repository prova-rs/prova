--- The self-hosting trampoline (docs/design/manifest.md#manifest-declared-runner): a manifest's
--- `[runner]` names which prova judges this package. Any prova invoked at the home provisions
--- the runner (`build`) and re-execs the declared `bin` with the original argv — freshness and
--- identity as mechanism, replacing the "never prove through an installed prova" prose.
---
--- The sandbox re-arms the trampoline by passing EMPTY guard vars: inside a proof, prova.bin
--- children carry PROVA_RUN_DEPTH/PROVA_TRAMPOLINED, and empty-counts-as-unset is the designed
--- seam that lets a nested prova exercise the hop.

local ARM = { PROVA_RUN_DEPTH = "", PROVA_TRAMPOLINED = "" }

local function armed(extra)
  local env = { PROVA_RUN_DEPTH = "", PROVA_TRAMPOLINED = "" }
  for k, v in pairs(extra or {}) do env[k] = v end
  return env
end

--- A package whose declared runner is a shell script: observable build, observable exec,
--- distinctive exit code — the hop made visible from outside.
local function runnered(root)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", [=[
[run]
proofs = ["proofs"]

[runner]
build = "echo provisioned >> build.log"
bin   = "fake-runner.sh"
]=])
  fs.write(proj .. "/fake-runner.sh", '#!/bin/sh\necho "RUNNER_OK $@"\nexit 7\n')
  shell.run({ "chmod", "+x", proj .. "/fake-runner.sh" }, { cwd = proj })
  fs.write(proj .. "/proofs/one_test.lua", [[
prova.test("green", function(t) t:expect(true):is_true() end)
]])
  return proj
end

local scratch = prova.fixture("runner-scratch", Scope.Test, function(ctx)
  return ctx:tempdir()
end)

prova.test("[runner] provisions, then re-execs: build first, the declared bin judges, argv and exit code forwarded", {
  covers = "docs/design/manifest.md#manifest-declared-runner",
  proves = "whichever prova was invoked, the one that judges is the one the manifest names — the Gradle Wrapper move, retiring freshness/identity from prose to mechanism",
  requires = { "unix" },
}, function(t)
  local proj = runnered(t:use(scratch))
  local r = shell.run(prova.bin .. " tests --anything you-like", { cwd = proj, env = armed(), merge_stderr = true })
  t:expect(r.stdout, "the declared bin ran"):contains("RUNNER_OK")
  t:expect(r.stdout, "the original argv crossed the hop"):contains("tests --anything you-like")
  t:expect(r.code, "the runner's exit code is the invocation's"):equals(7)
  t:expect(fs.read(proj .. "/build.log"), "the provision step ran first"):contains("provisioned")
end)

prova.test("the hop happens once: a trampolined child proceeds as the runner", {
  covers = "docs/design/manifest.md#manifest-declared-runner",
  proves = "the guard env rides the exec and inherits to every descendant — no rebuild storm under a live suite",
  requires = { "unix" },
}, function(t)
  local proj = runnered(t:use(scratch))
  -- Marked as the hop's child: no build, no exec — this binary IS the runner, and the ordinary
  -- machinery answers (the tests lane lists the sandbox's one proof).
  local r = shell.run(prova.bin .. " tests", { cwd = proj, env = armed({ PROVA_TRAMPOLINED = "1" }), merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):never():contains("RUNNER_OK")
  t:expect(r.stdout):contains("green")
  t:expect(fs.exists(proj .. "/build.log"), "no re-provision under the flag"):is_false()
end)

prova.test("a failed provision is loud (exit 2) and the runner is never exec'd", {
  covers = "docs/design/manifest.md#manifest-declared-runner",
  proves = "a build failure is a failed provision, not a verdict — and never a silent fallback to whatever binary happened to be invoked",
  requires = { "unix" },
}, function(t)
  local proj = runnered(t:use(scratch))
  fs.write(proj .. "/prova.toml", [=[
[run]
proofs = ["proofs"]

[runner]
build = "exit 1"
bin   = "fake-runner.sh"
]=])
  local r = shell.run(prova.bin .. " tests", { cwd = proj, env = armed(), merge_stderr = true })
  t:expect(r.code):equals(2)
  t:expect(r.stdout):contains("build failed")
  t:expect(r.stdout):never():contains("RUNNER_OK")
end)
