--- The verdict-ingestion seam (docs/design/verifiers.md), proven black-box: `junit.load` parses
--- the lingua franca into named cases, `junit.verify` conducts a deputy and adopts its verdict
--- (freshness-gated, vacuity-refusing), the ingested cases land in the run record as the deputed
--- account, and `prova attest junit:<suite>#<case>` answers against it.

-- A surefire-shaped document: 1 pass, 1 failure (with entities), 1 skip.
local MIXED_XML = [[<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.acme.OrderTest" tests="3" failures="1" skipped="1" time="0.31">
  <testcase name="drains" classname="com.acme.OrderTest" time="0.10"/>
  <testcase name="rejects" classname="com.acme.OrderTest" time="0.12">
    <failure message="expected 400 &amp; got 500" type="AssertionError"/>
  </testcase>
  <testcase name="windows only" classname="com.acme.OrderTest" time="0">
    <skipped/>
  </testcase>
</testsuite>]]

local GREEN_XML = [[<testsuite name="com.acme.GreenTest" tests="1">
  <testcase name="holds" classname="com.acme.GreenTest" time="0.01"/>
</testsuite>]]

local scratch = prova.fixture("junit-scratch", Scope.Test, function(ctx)
  return ctx:tempdir()
end)

prova.test("junit.load parses the stable core into named cases, never a blob", {
  covers = "docs/design/verifiers.md#ingest-structured",
  proves = "structured sub-verdicts are the difference between federating a deputy and grepping its stdout",
}, function(t)
  local dir = t:use(scratch)
  fs.write(dir .. "/results.xml", MIXED_XML)
  local report = junit.load(dir .. "/results.xml")
  t:expect(report.total):equals(3)
  t:expect(report.passed):equals(1)
  t:expect(report.failed):equals(1)
  t:expect(report.skipped):equals(1)
  t:expect(report.cases[1].suite):equals("com.acme.OrderTest")
  t:expect(report.cases[1].name):equals("drains")
  t:expect(report.cases[1].outcome):equals("passed")
  -- Entities decode — the message reaches the account as the deputy meant it.
  t:expect(report.cases[2].message):equals("expected 400 & got 500")
end)

prova.test("junit.verify adopts the verdict loudly, with the deputed cases' own names", {
  covers = "docs/design/verifiers.md#verifier-falsifiable",
  proves = "the negative control: a facet proven only on green fixtures is a rubber stamp — this drives the deputy red and asserts the facet surfaces it",
}, function(t)
  -- A nested sandbox whose proof conducts a fake deputy: the "runner" just copies a RED
  -- fixture into place, exactly the shape of `mvn test` writing surefire reports.
  local root = t:use(scratch)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/fixtures.xml", MIXED_XML)
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/deputy_test.lua", [[
prova.test("the deputy's suite holds", function(t)
  junit.verify(t, {
    run = "cp fixtures.xml out.xml",
    results = "out.xml",
    cwd = prova.root,
  })
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, "a red deputed case must fail the conducting proof"):never():equals(0)
  t:expect(r.stdout, "the failure names the deputed case"):contains("com.acme.OrderTest#rejects")
  t:expect(r.stdout, "with the deputy's own message"):contains("expected 400 & got 500")
end)

prova.test("deputed cases are never nodes: one conducting proof, not N test-shaped impostors", {
  covers = "docs/design/verifiers.md#deputed-not-nodes",
  proves = "the deputy owns selection and re-runs; prova owns the account — the same separate-account lesson reminders taught",
}, function(t)
  local root = t:use(scratch)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/fixtures.xml", GREEN_XML)
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/deputy_test.lua", [[
prova.test("the deputy's suite holds", function(t)
  junit.verify(t, { run = "cp fixtures.xml out.xml", results = "out.xml", cwd = prova.root })
end)
]])
  local l = shell.run(prova.bin .. " tests", { cwd = proj, merge_stderr = true })
  t:expect(l.stdout):contains("the deputy's suite holds")
  t:expect(l.stdout, "deputed cases are not collectible"):never():contains("GreenTest")
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the tally counts the conducting proof alone"):contains("1 passed, 0 failed")
end)

prova.test("ingested cases land in the record with provenance; attest answers deputed addresses", {
  covers = "docs/design/verifiers.md#deputed-in-record",
  proves = "'the deputy passed' becomes checkable case by case, from the same file attest already reads",
}, function(t)
  local root = t:use(scratch)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/fixtures.xml", GREEN_XML)
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/deputy_test.lua", [[
prova.test("the deputy's suite holds", function(t)
  junit.verify(t, { run = "cp fixtures.xml out.xml", results = "out.xml", cwd = prova.root })
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  local recorded = json.decode(fs.read(proj .. "/.prova/var/last-run.json"))
  t:expect(#recorded.deputed):equals(1)
  t:expect(recorded.deputed[1].verifier):equals("junit")
  t:expect(recorded.deputed[1].suite):equals("com.acme.GreenTest")
  t:expect(recorded.deputed[1].outcome):equals("passed")
  t:expect(recorded.deputed[1].file, "provenance rides along"):contains("out.xml")
end)

prova.test("`prova attest junit:<suite>#<case>` gates on the deputed account", {
  covers = "docs/design/verifiers.md#attest-deputed",
  proves = "the attestation question, one layer down: a red or absent deputed case attests nothing, one exit code for a pipeline",
}, function(t)
  local root = t:use(scratch)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/fixtures.xml", GREEN_XML)
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/deputy_test.lua", [[
prova.test("the deputy's suite holds", function(t)
  junit.verify(t, { run = "cp fixtures.xml out.xml", results = "out.xml", cwd = prova.root })
end)
]])
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local yes = shell.run(prova.bin .. ' attest "junit:com.acme.GreenTest#holds"',
    { cwd = proj, merge_stderr = true })
  t:expect(yes.code, yes.stdout):equals(0)
  t:expect(yes.stdout):contains("attested")
  local no = shell.run(prova.bin .. ' attest "junit:com.acme.GreenTest#imaginary"',
    { cwd = proj, merge_stderr = true })
  t:expect(no.code, "an absent case attests nothing"):never():equals(0)
  t:expect(no.stdout):contains("NOT attested")
end)

prova.test("freshness: a stale artifact is never adopted as this run's evidence", {
  covers = "docs/design/verifiers.md#verify-freshness",
  proves = "ingesting last week's results.xml is the green lie the run-and-ingest motion exists to forbid",
}, function(t)
  local dir = t:use(scratch)
  fs.write(dir .. "/old.xml", GREEN_XML)
  -- Age the artifact past the freshness window, then conduct a "deputy" that writes nothing.
  shell.run("touch -t 202001010000 " .. dir .. "/old.xml", { merge_stderr = true })
  local ok, err = pcall(function()
    junit.verify(t, { run = "true", results = dir .. "/old.xml" })
  end)
  t:expect(ok, "stale results must refuse, not pass"):is_false()
  t:expect(tostring(err)):contains("predates the run")
end)

prova.test("vacuity: zero parsed cases fails — a wrong glob must never read as green", {
  covers = "docs/design/verifiers.md#two-provenances",
  proves = "deputed evidence is held to the observed bar: 'the glob matched nothing' reading green would be the vacuous pass one tool further out",
}, function(t)
  local root = t:use(scratch)
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/deputy_test.lua", [[
prova.test("the deputy's suite holds", function(t)
  junit.verify(t, { results = prova.root .. "/nowhere/*.xml" })
end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, "zero cases must fail the conducting proof"):never():equals(0)
  t:expect(r.stdout):contains("zero cases parsed")
end)

prova.test("the binary teaches the seam: `prova learn verifiers` names the contract", function(t)
  local r = shell.run(prova.bin .. " learn verifiers", { merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("junit.verify")
  t:expect(r.stdout):contains("deputed")
end)
