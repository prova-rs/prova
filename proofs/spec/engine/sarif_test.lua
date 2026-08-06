-- Spec for the sarif findings seam (docs/design/verifiers.md). sarif.load parses SARIF (the linter/
-- static-analysis interchange) and sarif.verify adopts a linter's verdict. Fixtures are written
-- inline; the red cases run through prova.bin so a failing assertion is observed as a non-zero exit
-- rather than failing this test.

local function sarif_doc(results)
  return json.encode({
    version = "2.1.0",
    runs = { { tool = { driver = { name = "clippy" } }, results = results } },
  })
end

local ERROR_RESULT = {
  ruleId = "clippy::correctness",
  level = "error",
  message = { text = "this will panic" },
  locations = { { physicalLocation = { artifactLocation = { uri = "src/lib.rs" }, region = { startLine = 7 } } } },
}
local WARN_RESULT = {
  ruleId = "clippy::style",
  level = "warning",
  message = { text = "prefer x" },
  locations = { { physicalLocation = { artifactLocation = { uri = "src/main.rs" }, region = { startLine = 3 } } } },
}

prova.test("sarif.load parses findings and counts levels", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/out.sarif", sarif_doc({ WARN_RESULT, ERROR_RESULT }))
  local report = sarif.load(dir .. "/out.sarif")
  t:expect(report.total):equals(2)
  t:expect(report.errors):equals(1)
  t:expect(report.warnings):equals(1)
  t:expect(report.cases[2].rule):equals("clippy::correctness")
  t:expect(report.cases[2].line):equals(7)
end)

prova.test("sarif.verify passes on a clean report and on warnings-only (default gate is errors)", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/clean.sarif", sarif_doc({}))
  sarif.verify(t, { results = dir .. "/clean.sarif" }) -- zero findings: a clean run is green
  fs.write(dir .. "/warn.sarif", sarif_doc({ WARN_RESULT }))
  sarif.verify(t, { results = dir .. "/warn.sarif" }) -- warning < error threshold: still green
end)

prova.test("sarif.verify fails on an error-level finding", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(dir .. "/err.sarif", sarif_doc({ ERROR_RESULT }))
  fs.write(dir .. "/proofs/lint_test.lua",
    'prova.test("lint", function(t) sarif.verify(t, { results = prova.root .. "/err.sarif" }) end)\n')
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("clippy::correctness")
end)

prova.test("a warning becomes red when the threshold is lowered to 'warning'", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(dir .. "/warn.sarif", sarif_doc({ WARN_RESULT }))
  fs.write(dir .. "/proofs/lint_test.lua",
    'prova.test("lint", function(t) sarif.verify(t, { results = prova.root .. "/warn.sarif", level = "warning" }) end)\n')
  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
end)
