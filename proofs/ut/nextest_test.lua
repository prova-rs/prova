--- The deputed unit-test leg (docs/design/verifiers.md#conduct-once-read-many): `prova run ut`
--- conducts cargo nextest ONCE for the whole workspace — compilation and execution both live in a
--- file-scoped fixture that emits one junit artifact — then one proof adopts every case into the
--- account, and sibling readers bind claims to named cases at zero additional compilations.
---
--- HEAVY: conducting compiles the workspace, so this must never fire because a person typed
--- `prova`. The whole file sits behind the `ut` switch (suite.lua) — off unless thrown, thrown by
--- the `ut` profile or `-s ut` — while `cargo-nextest` stays a `requires` world fact: intent and
--- world are two facts with two remedies (docs/design/manifest.md#switches-not-env-capabilities).
--- The profile `must_run`s the deputy, so `prova run ut` fails rather than skips when nextest is
--- missing: a profile is a contract, not a courtesy.

-- Conduct the deputy once. The stale artifact is removed FIRST, so a deputy that dies before
-- emitting (a compile error) leaves nothing behind and the adoption fails loudly on "matched
-- nothing" — never a previous run's verdicts wearing this run's face. The deputy's exit code is
-- deliberately not asserted here: the adopting proof reports red with the deputed cases' own
-- names, which a fixture death would hide.
local deputy = prova.fixture("nextest-junit", Scope.File, function()
  local artifact = prova.root .. "/target/nextest/prova/junit.xml"
  fs.remove_all(artifact)
  shell.run(
    { "cargo", "nextest", "run", "--workspace", "--profile", "prova" },
    { cwd = prova.root, merge_stderr = true, timeout = "900s" }
  )
  return artifact
end)

prova.test("the workspace's unit-test account holds — every nextest case adopted", {
  requires = { "cargo-nextest" },
  covers = "docs/design/verifiers.md#conduct-once-read-many",
  proves = "one compilation feeds the whole adoption: the fixture conducts, this proof ledgers every case as deputed rows, and the readers below pay only a parse",
}, function(t)
  junit.verify(t, { results = t:use(deputy) })
end)

-- Readers: one claim, one named unit test. This is the granularity the pattern exists for — a
-- prose claim discharged by a specific case in the deputy's own account, without a second run.
local function case(report, name)
  for _, c in ipairs(report.cases) do
    if c.name == name then
      return c
    end
  end
end

local function expect_passed(t, report, name)
  local c = case(report, name)
  t:expect(c, name .. " exists in the deputed account"):is_truthy()
  t:expect(c.outcome, name):equals("passed")
end

prova.test("the lane alignment invariants hold, spoken for by the account", {
  requires = { "cargo-nextest" },
  covers = "docs/design/mcp-mode.md#mcp-cli-parity",
  proves = "increment 1 deferred exactly this: the parity unit tests existed but evidence/owed/attest could not speak for them — now the account adopts their verdicts",
}, function(t)
  local report = junit.load(t:use(deputy))
  expect_passed(t, report, "tests::lane_surface_parity")
  expect_passed(t, report, "tests::mcp_tools_are_real_verbs")
end)

prova.test("selection-axes parity holds at the unit level, bound to its claim", {
  requires = { "cargo-nextest" },
  covers = "docs/design/mcp-mode.md#selection-axes-parity",
  proves = "the structural half of the parity claim (the exhaustive destructure + wire-schema match) reaches the account beside the behavioral black-box half",
}, function(t)
  local report = junit.load(t:use(deputy))
  expect_passed(t, report, "mcp::tests::selection_axes_parity")
end)
