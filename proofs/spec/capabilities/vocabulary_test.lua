--- Prova testing Prova: `[capabilities]` — the declared vocabulary of host facts
--- (docs/design/capabilities.md), and the `prova.lua` companion it replaced.
---
--- A capability is a name plus a factory, declared in the manifest, in one of three kinds:
---
---   package    a Lua predicate exported from a package — resolved EAGERLY, at load
---   command    a declarative probe (command/probe/expect/version/stream/pattern) — LAZY, memoized
---   intrinsic  one of prova's own checkers, named out loud
---
--- Two invariants shape every test below, and neither is a preference:
---
---   ORDERING. `must_run` is a PRECONDITION, checked before any proof file loads. That is why the
---     vocabulary lives in the manifest (plus the resolved package set) and why there is no
---     proof-file form of a capability declaration — one would not exist yet at the moment it is
---     needed. §D is the real reason this file exists; the rest is the surface.
---   ANSWERS, NOT CLOSURES. A package predicate runs ONCE, at load, and only its verdict survives.
---     A capability that answered differently for two suites in one run would be a coin flip.

local prova_bin = assert(prova.bin, "prova.bin not injected by the runtime")

--- A scratch project. `caps` is the `[capabilities]` body; `manifest_extra` is appended verbatim;
--- `predicates` (when given) becomes an `env` package exporting capability functions.
local function project(caps, manifest_extra, predicates)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]',
    'proofs = ["."]',
    'packages = "packages"',
    '[luals]',
    'manage = "never"',
    caps and ('[capabilities]\n' .. caps) or '',
    manifest_extra or '',
  }, "\n"))
  if predicates then
    fs.write(dir .. "/packages/env/prova.toml", '[package]\nname = "env"\n')
    fs.write(dir .. "/packages/env/init.lua", predicates)
  end
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("plain", function(t) t:expect(1):equals(1) end)',
    'prova.test("needs gpu", { requires = { "gpu" } }, function(t) t:expect(1):equals(1) end)',
  }, "\n"))
  return dir
end

--- A predicate body that RECORDS that it ran, then answers `verdict`.
---
--- Load-bearing: without proof of execution, "declared and answered false" is indistinguishable from
--- "never declared" — both skip, both name `gpu`, both fail must_run. Every one of those assertions
--- passed against the unimplemented feature until the marker was added.
local function predicate(verdict)
  return 'local M = { capabilities = {} }\n'
      .. 'function M.capabilities.gpu()\n'
      .. '  fs.write(os.getenv("PROVA_SELFTEST_MARK"), "ran")\n'
      .. '  return ' .. verdict .. '\n'
      .. 'end\n'
      .. 'return M\n'
end

------------------------------------------------------------------------------------------
-- A. The package selector — a Lua predicate, from a package, testable
------------------------------------------------------------------------------------------

prova.test("no [capabilities] behaves exactly as before", function(t)
  -- The section is OPTIONAL. Absent → `gpu` is an unknown capability, probed on PATH and not found,
  -- so the test that needs it skips.
  local dir = project(nil)
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("plain")
  t:expect(r.stdout, "an undeclared capability is just unavailable"):contains("skipped")
end)

prova.test("a package predicate that holds makes the test RUN", {
  covers = "docs/design/capabilities.md#predicate-lives-in-a-package",
}, function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil, predicate("true"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "gpu is available, so the gated test ran"):contains("2 passed, 0 failed, 0 skipped")
  t:expect(fs.exists(mark), "the predicate ran"):is_true()
end)

prova.test("a package predicate that does NOT hold skips the test", function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil, predicate("false"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code, "an unmet requirement is a skip, never a failure"):equals(0)
  -- The marker is what makes this mean anything: it proves the predicate RAN and said no, rather
  -- than the capability having been unknown all along (which skips identically).
  t:expect(fs.exists(mark), "the declared predicate actually ran"):is_true()
  t:expect(r.stdout, "exactly the gated test skipped"):contains("1 passed, 0 failed, 1 skipped")
end)

prova.test("`capability` resolves through the package's `capabilities` namespace", {
  covers = "docs/design/capabilities.md#the-capabilities-namespace-is-the-advertisement",
  proves = "a package publishes a capability by exporting it under `capabilities`, and the consumer \
names only that — no advertisement table, and no dotted path in the consumer's manifest",
}, function(t)
  -- The convention IS the contract: `capability = "gpu"` finds `capabilities.gpu` and nothing else.
  -- A function outside that namespace is unreachable by the encapsulated form.
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil,
    'local M = { capabilities = {} }\n'
    .. 'function M.gpu() return true end\n'    -- top level, NOT under `capabilities`
    .. 'return M\n')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "a capability whose factory is not in the namespace is a config error"):equals(2)
  t:expect(r.stdout .. r.stderr, "the error names the capability"):contains("gpu")
end)

prova.test("`factory` reaches a path outside the conventional namespace", function(t)
  local dir = project('gpu = { package = "env", factory = "probes.gpu" }', nil,
    'local M = { probes = {} }\nfunction M.probes.gpu() return true end\nreturn M\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("2 passed, 0 failed, 0 skipped")
end)

prova.test("`options` reach the factory as its argument", {
  proves = "one generic factory can serve several capabilities, which is what `options` is for",
}, function(t)
  local dir = project('gpu = { package = "env", capability = "gpu", options = { tier = "pro" } }', nil,
    'local M = { capabilities = {} }\n'
    .. 'function M.capabilities.gpu(opts) return opts ~= nil and opts.tier == "pro" end\n'
    .. 'return M\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the factory saw its options"):contains("2 passed, 0 failed, 0 skipped")
end)

prova.test("a predicate is evaluated ONCE per run, not per test", function(t)
  -- Two gated tests, one predicate. A capability that answered per-test could answer differently
  -- per test, which is not a capability — it is a coin flip. (It also could not be checked as a
  -- precondition, since that happens before any test exists.)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["."]', 'packages = "packages"',
    '[luals]', 'manage = "never"',
    '[capabilities]', 'counted = { package = "env", capability = "counted" }',
  }, "\n"))
  fs.write(dir .. "/packages/env/prova.toml", '[package]\nname = "env"\n')
  fs.write(dir .. "/packages/env/init.lua", table.concat({
    'local M = { capabilities = {} }',
    'local n = 0',
    'function M.capabilities.counted()',
    '  n = n + 1',
    '  fs.write(os.getenv("PROVA_SELFTEST_COUNT_FILE"), tostring(n))',
    '  return true',
    'end',
    'return M',
  }, "\n"))
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("a", { requires = { "counted" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("b", { requires = { "counted" } }, function(t) t:expect(1):equals(1) end)',
  }, "\n"))
  local counter = dir .. "/count.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_COUNT_FILE = counter } })
  t:expect(r.code):equals(0)
  t:expect(fs.read(counter), "one evaluation, two consumers"):equals("1")
end)

prova.test("a predicate is an ordinary function a PROOF can call", {
  covers = "docs/design/capabilities.md#predicate-lives-in-a-package",
  proves = "the original complaint: a capability predicate used to live in a file the runtime loaded \
for itself, so nothing could assert on it. In a package it is just an exported function.",
}, function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil,
    'local M = { capabilities = {} }\nfunction M.capabilities.gpu() return "2.4.0" end\nreturn M\n')
  -- A proof in the same project requires the package and calls the predicate directly.
  fs.write(dir .. "/direct_test.lua", table.concat({
    'local env = require("env")',
    'prova.test("the predicate is callable and reports a version", function(t)',
    '  t:expect(env.capabilities.gpu()):equals("2.4.0")',
    'end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "the predicate is testable like any other function"):equals(0)
  t:expect(r.stdout):contains("the predicate is callable and reports a version")
end)

------------------------------------------------------------------------------------------
-- B. The command selector — the declarative probe
------------------------------------------------------------------------------------------

prova.test("a bare `command` is PATH presence plus the --version heuristic", {
  requires = { "unix" },
  proves = "`{ command = \"x\" }` is exactly what an undeclared `x` already did, written down — so \
declaring a tool is never a behavior change, only a statement of intent",
}, function(t)
  local dir = project('shell = { command = "sh" }\nabsent = { command = "definitely-not-a-tool-xyz" }')
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("present", { requires = { "shell" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("absent", { requires = { "absent" } }, function(t) error("must not run") end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 passed, 0 failed, 1 skipped")
end)

prova.test("`probe` + `expect` is the availability half: it must answer AND say the right thing", {
  requires = { "unix" },
  proves = "the shape that catches a Windows-container docker daemon — `docker info` succeeds and \
the answer is still wrong, so exit code alone cannot be the gate",
}, function(t)
  local dir = project(table.concat({
    'yes = { command = "sh", probe = ["-c", "echo linux"], expect = "linux", version = false }',
    'no  = { command = "sh", probe = ["-c", "echo windows"], expect = "linux", version = false }',
  }, "\n"))
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("expected", { requires = { "yes" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("unexpected", { requires = { "no" } }, function(t) error("must not run") end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "a command that answers the WRONG thing is unavailable"):contains("1 passed, 0 failed, 1 skipped")
end)

prova.test("`stream = \"stderr\"` reads a version the old heuristic could not see", {
  requires = { "unix" },
  proves = "the concrete gap that made a declarative probe worth having: `java -version` reports on \
stderr, and nothing in the built-in vocabulary could reach it without writing Lua in a special file",
}, function(t)
  local dir = project(
    'tool = { command = "sh", version = ["-c", "echo \'tool version 4.5.6\' 1>&2"], stream = "stderr" }')
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("satisfied", { requires = { "tool >= 4.0" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("too new", { requires = { "tool >= 9.0" } }, function(t) error("must not run") end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 passed, 0 failed, 1 skipped")
  t:expect(r.stdout, "the skip names the version it found on stderr"):contains("4.5.6")
end)

prova.test("`pattern` picks the right number out of structure", {
  requires = { "unix" },
  proves = "`kubectl version --client` prints several versions; the heuristic would take the first. \
The pattern narrows and the parser still normalizes, so a pattern need not produce strict semver.",
}, function(t)
  -- One line: TOML inline tables may not span newlines.
  local dir = project(
    'k = { command = "sh", version = ["-c", "echo \'Kustomize Version: v5.0.4\'; ' ..
    'echo \'Client Version: v1.30.2\'"], pattern = "Client Version: v([0-9.]+)" }')
  fs.write(dir .. "/x_test.lua",
    'prova.test("client version, not the first one", { requires = { "k >= 1.30, < 2.0" } }, ' ..
    'function(t) t:expect(1):equals(1) end)')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the pattern found 1.30.2, not 5.0.4"):contains("1 passed, 0 failed, 0 skipped")
end)

prova.test("`version = false` makes a constraint unsatisfiable, never satisfied", {
  requires = { "unix" },
  covers = "docs/design/capabilities.md#version-false-cannot-satisfy-a-constraint",
}, function(t)
  local dir = project('tool = { command = "sh", version = false }')
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("bare need", { requires = { "tool" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("versioned need", { requires = { "tool >= 1.0" } }, function(t) error("must not run") end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  -- Available, but "is it >= 1.0?" cannot be confirmed — and a gate that cannot confirm must not
  -- wave the suite through.
  t:expect(r.stdout):contains("1 passed, 0 failed, 1 skipped")
  t:expect(r.stdout):contains("cannot be confirmed")
end)

prova.test("a malformed `pattern` is a config error, not a version that never parses", function(t)
  local dir = project('tool = { command = "sh", pattern = "([0-9" }')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  -- A typo'd regex that silently never matched would read as "cannot confirm" and skip forever,
  -- which reads as green — the vacuous green this whole contract exists to remove.
  t:expect(r.code):equals(2)
  t:expect(r.stdout .. r.stderr):contains("regex")
end)

------------------------------------------------------------------------------------------
-- C. Selectors, intrinsics, and overrides
------------------------------------------------------------------------------------------

prova.test("an entry with no selector, or two, is refused", {
  covers = "docs/design/capabilities.md#exactly-one-selector",
}, function(t)
  local none = shell.run(prova_bin, { cwd = project('x = { retries = 2 }'), merge_stderr = true })
  t:expect(none.code):equals(2)
  t:expect(none.stdout .. none.stderr):contains("selector")

  local two = shell.run(prova_bin,
    { cwd = project('x = { command = "sh", intrinsic = "docker" }'), merge_stderr = true })
  t:expect(two.code, "two selectors is an author who is unsure, not a precedence question"):equals(2)
  t:expect(two.stdout .. two.stderr):contains("selector")
end)

prova.test("`intrinsic` names one of prova's own checkers, and can alias it", function(t)
  local dir = project('dockerd = { intrinsic = "docker" }')
  fs.write(dir .. "/x_test.lua",
    'prova.test("aliased", { requires = { "dockerd" } }, function(t) t:expect(1):equals(1) end)')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  -- Host-agnostic: with a daemon it runs, without one it skips. Either way it must not ERROR, which
  -- is what an unresolvable intrinsic would do.
  t:expect(r.code, "an aliased built-in resolves"):equals(0)
end)

prova.test("an unknown `intrinsic` is refused, naming the real ones", function(t)
  local r = shell.run(prova_bin,
    { cwd = project('x = { intrinsic = "dokcer" }'), merge_stderr = true })
  -- A typo that resolved to "absent" would skip every gated test forever while reading as a
  -- deliberate declaration.
  t:expect(r.code):equals(2)
  t:expect(r.stdout .. r.stderr, "the error lists the built-ins"):contains("docker")
end)

prova.test("a declaration may OVERRIDE a built-in, and the report says so", {
  requires = { "unix" },
  covers = {
    "docs/design/capabilities.md#overriding-a-builtin-is-declared",
    -- The run and the report agree about what `docker` means here, which is the observable form of
    -- "one resolution point": two surfaces, one `Capabilities`, no way for an override to be
    -- honored by one and not the other.
    "docs/design/capabilities.md#one-resolution-point",
  },
  proves = "the old blanket refusal was protecting against a SILENT override — a predicate in a file \
nobody reads. A manifest entry is not silent, so the refusal became a report instead of a ban.",
}, function(t)
  local dir = project('docker = { command = "sh", version = false }')
  fs.write(dir .. "/x_test.lua",
    'prova.test("docker means sh here", { requires = { "docker" } }, function(t) t:expect(1):equals(1) end)')
  local run = shell.run(prova_bin, { cwd = dir })
  t:expect(run.code):equals(0)
  t:expect(run.stdout, "the override is honored at run time"):contains("1 passed, 0 failed, 0 skipped")
  local report = shell.run(prova_bin .. " capabilities", { cwd = dir, merge_stderr = true })
  t:expect(report.stdout, "and never printed as an ordinary row"):contains("OVERRIDES the built-in")
end)

prova.test("a declared NO is final — it never falls through to a PATH hit", {
  requires = { "unix" },
  covers = "docs/design/capabilities.md#a-declared-no-is-final",
  proves = "the companion's latent bug: a predicate returning false left the name unregistered, so a \
binary of that name on PATH could still answer YES about a capability its own factory refused",
}, function(t)
  -- `sh` is on PATH everywhere this runs, so only a declared no can make it unavailable.
  local dir = project('sh = { package = "env", capability = "gpu" }', nil, predicate("false"))
  fs.write(dir .. "/x_test.lua",
    'prova.test("must not run", { requires = { "sh" } }, function(t) error("must not run") end)')
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code):equals(0)
  t:expect(fs.exists(mark), "the predicate ran and said no"):is_true()
  t:expect(r.stdout, "and its no beat the PATH probe"):contains("0 passed, 0 failed, 1 skipped")
end)

------------------------------------------------------------------------------------------
-- D. must_run — the ORDERING proof, and the reason the vocabulary is manifest-level
------------------------------------------------------------------------------------------

prova.test("must_run can guarantee a DECLARED capability", {
  proves = "the structural point: `must_run` is checked before any proof file loads, so this only \
works because [capabilities] resolves with the manifest. A capability declared in a proof file \
could never be guaranteed — it would not exist yet.",
}, function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }',
    '\n[profiles.ci]\nmust_run = ["gpu"]\n', predicate("true"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code, "a declared capability is guaranteeable"):equals(0)
  t:expect(fs.exists(mark), "the precondition saw a capability the manifest declared"):is_true()
end)

prova.test("must_run FAILS when a declared capability does not hold", function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }',
    '\n[profiles.ci]\nmust_run = ["gpu"]\n', predicate("false"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code, "an unmet guarantee fails, declared or built-in"):equals(2)
  -- Without the marker this passes against an unimplemented feature: an UNKNOWN gpu also fails.
  t:expect(fs.exists(mark), "it failed because the predicate said no, not because gpu was unknown"):is_true()
  t:expect(r.stderr .. r.stdout):contains("gpu")
end)

prova.test("must_run on an undeclared capability still fails", function(t)
  -- No `[capabilities]` at all: `gpu` is unknown, so the guarantee cannot be honored. A typo'd
  -- capability in must_run must not pass silently.
  local dir = project(nil, '\n[profiles.ci]\nmust_run = ["gpu"]\n')
  local r = shell.run(prova_bin .. " --profile ci", { cwd = dir })
  t:expect(r.code):equals(2)
end)

prova.test("a version-reporting predicate composes with the same expression grammar", function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil, predicate('"2.4.0"'))
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("ok", { requires = { "gpu >= 2.0" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("too new", { requires = { "gpu >= 9.0" } }, function(t) error("must not run") end)',
  }, "\n"))
  local r = shell.run(prova_bin .. " --format json",
    { cwd = dir, env = { PROVA_SELFTEST_MARK = dir .. "/mark.txt" } })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "reports the found version, not just the constraint"):contains("2.4.0")
end)

------------------------------------------------------------------------------------------
-- E. The `"*"` fall-through policy — strictness as a ratchet
------------------------------------------------------------------------------------------

prova.test("the default fall-through probes PATH, exactly as before", {
  covers = "docs/design/capabilities.md#wildcard-declares-the-fall-through",
  requires = { "unix" },
}, function(t)
  local dir = project('nothing_declared = { command = "sh" }')
  fs.write(dir .. "/x_test.lua",
    'prova.test("undeclared but present", { requires = { "sh" } }, function(t) t:expect(1):equals(1) end)')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "`requires = { \"sh\" }` needs no ceremony"):contains("1 passed, 0 failed, 0 skipped")
end)

prova.test("\"*\" = \"error\" FAILS an undeclared name — it does not skip it", {
  covers = "docs/design/capabilities.md#wildcard-declares-the-fall-through",
  proves = "a closed vocabulary must be a GATE. Routed through the skip path, `error` would produce \
a green run with an explanatory skip — a noisier `warn`, and the vacuous green in a new costume.",
}, function(t)
  local dir = project('"*" = "error"\nsh = { command = "sh" }')
  fs.write(dir .. "/x_test.lua", table.concat({
    'prova.test("declared", { requires = { "sh" } }, function(t) t:expect(1):equals(1) end)',
    'prova.test("mystery", { requires = { "kubectl" } }, function(t) t:expect(1):equals(1) end)',
  }, "\n"))
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "an undeclared name under a closed vocabulary is RED"):equals(1)
  local out = r.stdout .. r.stderr
  t:expect(out, "the error teaches the declaration"):contains("[capabilities]")
  t:expect(out, "and names what was undeclared"):contains("kubectl")
  -- Not a skip: the phrasing a skip would carry must be absent, or this passes against the very
  -- behavior it exists to reject.
  t:expect(out:match("skipped: requires \"kubectl\""), "a config error must not read as a skip"):is_nil()
end)

prova.test("\"*\" = \"error\" exempts prova's own built-ins", {
  covers = "docs/design/capabilities.md#strict-governs-only-undefined-names",
  proves = "strictness is about names the PACKAGE left unnailed-down. A bare `unix` is nailed down — \
prova defines it — so forcing six lines of intrinsic declarations before a strict package can say \
`requires = { \"unix\" }` would cost ergonomics and buy nothing.",
}, function(t)
  local dir = project('"*" = "error"')
  fs.write(dir .. "/x_test.lua",
    'prova.test("built-in", { requires = { "unix" } }, function(t) t:expect(1):equals(1) end)')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "a built-in needs no declaration even under a closed vocabulary"):equals(0)
end)

prova.test("\"*\" = \"warn\" runs, and teaches the missing declarations as a block", {
  requires = { "unix" },
  proves = "the migration rung: run warm, collect the lines, declare what they name, close the door. \
Without it, closing a vocabulary is a flag day rather than a ratchet.",
}, function(t)
  local dir = project('"*" = "warn"')
  fs.write(dir .. "/x_test.lua",
    'prova.test("undeclared", { requires = { "sh" } }, function(t) t:expect(1):equals(1) end)')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "warn probes and runs — it is not a gate"):equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out, "the teaching names the capability"):contains("sh = {")
  t:expect(out, "and hands back pasteable TOML"):contains("[capabilities]")
end)

prova.test("an unknown \"*\" policy is refused, not defaulted", function(t)
  local r = shell.run(prova_bin, { cwd = project('"*" = "strict"'), merge_stderr = true })
  -- Reading `"strict"` as `probe` would hand back the permissive behavior under a key that says
  -- otherwise — the exact failure a closed vocabulary exists to prevent.
  t:expect(r.code):equals(2)
  t:expect(r.stdout .. r.stderr):contains("fall-through policy")
end)

------------------------------------------------------------------------------------------
-- F. The deprecation bridge — the companion still works, and teaches its replacement
------------------------------------------------------------------------------------------

prova.test("the prova.lua companion still registers, and teaches the replacement", {
  proves = "a bridge, not a removal: existing projects keep running while every registration names \
the TOML that replaces it (docs/design/deprecations.md#retire-capability-companion)",
}, function(t)
  local dir = project(nil)
  fs.write(dir .. "/prova.lua", 'runtime.capability("gpu", function() return true end)\n')
  local r = shell.run(prova_bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "the companion still works"):equals(0)
  t:expect(r.stdout, "and its capability still gates"):contains("2 passed, 0 failed, 0 skipped")
  local out = r.stdout .. r.stderr
  t:expect(out, "the warning teaches the replacement section"):contains("[capabilities]")
  t:expect(out, "and names the capability to move"):contains("gpu")
end)

prova.test("[capabilities] wins over the companion, and says so", {
  covers = "docs/design/capabilities.md#manifest-wins-over-the-companion",
  proves = "silent precedence between a deprecated and a current mechanism is how a migration \
produces a mystery: the author edits the companion, nothing changes, and nothing said why",
}, function(t)
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil, predicate("true"))
  -- The companion says NO; the manifest says yes. The manifest is the current mechanism.
  fs.write(dir .. "/prova.lua", 'runtime.capability("gpu", function() return false end)\n')
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin, { cwd = dir, env = { PROVA_SELFTEST_MARK = mark }, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the manifest's answer is the one that counts"):contains("2 passed, 0 failed, 0 skipped")
  t:expect(r.stdout .. r.stderr, "and the shadowing is announced"):contains("both")
end)

prova.test("a broken prova.lua is an ERROR, not a silent skip", function(t)
  -- The failure this closes: a companion that failed to load would leave every capability it meant
  -- to register silently unregistered, so every gated test would skip — and the run would be green.
  local dir = project(nil)
  fs.write(dir .. "/prova.lua", 'this is not lua((((\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code, "a broken companion is a config error"):equals(2)
  t:expect(r.stderr .. r.stdout):contains("prova.lua")
end)

prova.test("registering over a built-in in the COMPANION is still refused", {
  proves = "the companion is the silent path, and silence is what made overriding dangerous. The \
manifest may override a built-in because a reader can see it there; a Lua file nobody reads may not.",
}, function(t)
  local dir = project(nil)
  fs.write(dir .. "/prova.lua", 'runtime.capability("docker", function() return true end)\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(2)
  t:expect(r.stderr .. r.stdout):contains("docker")
end)

prova.test("runtime.capability in a test raises a clear error, not a nil", function(t)
  -- The boundary the `runtime` namespace exists to make self-evident: it configures the environment
  -- tests run IN, so it is not available while a test runs. A metatable turns any `runtime.*` access
  -- in this state into a message pointing at prova.lua, rather than a baffling "call a nil value".
  local ok, err = pcall(function() runtime.capability("x", function() return true end) end)
  t:expect(ok, "calling it from a test must fail"):is_false()
  t:expect(tostring(err), "and the error must point at prova.lua"):contains("prova.lua")
end)

------------------------------------------------------------------------------------------
-- G. `prova capabilities <name>` — the explain form
------------------------------------------------------------------------------------------

prova.test("`capabilities <name>` shows what ran and what came back", {
  requires = { "unix" },
  proves = "the diagnostic gap this closes: an unmet capability used to report only that it was \
unavailable, and a wrong-version skip only the numbers — with no way to see which command produced \
them. The version came from somewhere, and `somewhere` was unprintable.",
}, function(t)
  local dir = project(
    'tool = { command = "sh", version = ["-c", "echo \'tool version 4.5.6\'"] }')
  local r = shell.run(prova_bin .. " capabilities tool", { cwd = dir, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the kind of declaration"):contains("command probe")
  t:expect(r.stdout, "the command it ran for the version"):contains("tool version 4.5.6")
  t:expect(r.stdout, "the status"):contains("MET")
  t:expect(r.stdout, "and the parsed version"):contains("4.5.6")
end)

------------------------------------------------------------------------------------------
-- H. Resolution timing, and the selectors that deliberately do not exist
------------------------------------------------------------------------------------------

prova.test("a declared command capability nothing requires is never probed", {
  requires = { "unix" },
  covers = "docs/design/capabilities.md#lua-eager-command-lazy",
  proves = "the reason laziness is worth machinery: under a closed vocabulary a serious package \
declares every tool it touches, and eagerly probing twenty of them would add twenty process spawns \
to every invocation — including `prova --list` — to answer questions nothing asked",
}, function(t)
  local dir = fs.tempdir()
  local mark = dir .. "/probed.txt"
  -- The probe RECORDS that it ran. Nothing in the suite requires `unused`, so it must never run.
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["."]', '[luals]', 'manage = "never"',
    '[capabilities]',
    'unused = { command = "sh", probe = ["-c", "echo ran > ' .. mark .. '"], version = false }',
  }, "\n"))
  fs.write(dir .. "/x_test.lua", 'prova.test("plain", function(t) t:expect(1):equals(1) end)')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(fs.exists(mark), "an unreferenced command capability must not be probed"):is_false()

  -- And it IS probed the moment something asks — otherwise this would pass against a probe that
  -- never works at all.
  fs.write(dir .. "/x_test.lua",
    'prova.test("asks", { requires = { "unused" } }, function(t) t:expect(1):equals(1) end)')
  local asked = shell.run(prova_bin, { cwd = dir })
  t:expect(asked.code):equals(0)
  t:expect(fs.exists(mark), "and it IS probed on first reference"):is_true()
end)

prova.test("a Lua predicate resolves EAGERLY, before any proof runs", {
  covers = "docs/design/capabilities.md#lua-eager-command-lazy",
  proves = "the other half of the timing split, and it is not a choice: mlua handles are !Send, each \
suite gets its own state, and `must_run` is checked before any suite exists — so a package predicate \
runs at load and only its verdict survives",
}, function(t)
  -- `--list` executes no test body. A Lua predicate still runs, because resolution happens with the
  -- manifest rather than on first reference.
  local dir = project('gpu = { package = "env", capability = "gpu" }', nil, predicate("true"))
  local mark = dir .. "/mark.txt"
  local r = shell.run(prova_bin .. " --list", { cwd = dir, env = { PROVA_SELFTEST_MARK = mark } })
  t:expect(r.code):equals(0)
  t:expect(fs.exists(mark), "the predicate ran at load, with no test body executed"):is_true()
end)

prova.test("there is deliberately no env-var selector", {
  covers = "docs/design/capabilities.md#intent-is-a-switch-not-a-capability",
  proves = "an env-var-probing capability is exactly the pattern switches replaced \
(docs/design/manifest.md#switches-not-env-capabilities); offering it as a declarative kind would \
rebuild the thing that was torn down. `[capabilities]` denies unknown keys, so the door stays shut.",
}, function(t)
  local r = shell.run(prova_bin, { cwd = project('ci = { env = "CI" }'), merge_stderr = true })
  t:expect(r.code, "an `env` selector is an unknown key, not a quiet no-op"):equals(2)
  t:expect(r.stdout .. r.stderr, "and the refusal names it"):contains("env")
end)

prova.test("a capability cannot be declared from a proof file", {
  covers = "docs/design/capabilities.md#capabilities-have-one-door",
  proves = "topologies have two doors — a proof-file `prova.topology` is a fixture — and capabilities \
have ONE. A file-local capability would be a name meaning different things in different files, which \
is the thing a capability exists to rule out; and `must_run` is checked before any proof file loads, \
so it could not exist yet in any case.",
}, function(t)
  -- There is no authoring API for it, and the one that used to exist (the companion's `runtime`)
  -- refuses from a test state with a message naming where it belonged.
  local ok, err = pcall(function() runtime.capability("invented", function() return true end) end)
  t:expect(ok, "no proof-file form exists"):is_false()
  t:expect(tostring(err)):contains("prova.lua")
  -- And a name a proof merely mentions is not thereby declared: it falls through to a PATH probe.
  local dir = project(nil)
  fs.write(dir .. "/x_test.lua",
    'prova.test("invented", { requires = { "a-name-no-manifest-declares" } }, ' ..
    'function(t) error("must not run") end)')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "an undeclared name is unavailable, never self-declared"):contains("1 skipped")
end)
