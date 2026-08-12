--- Selection pushdown (docs/design/verifiers.md#selection-pushdown-into-conducts): the engine
--- exposes the run's resolved selection as plain data (`prova.selection`), the deputy translates
--- it to its framework's grammar in its own package, and a narrowed run's deputed account says
--- NARROWED wherever it is read. The live exemplar of the translation is the workspace's own
--- nextest deputy (.prova/packages/deputies); these proofs pin the engine's half hermetically.

local function package(t, files)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  for name, body in pairs(files) do
    fs.write(proj .. "/proofs/" .. name, body)
  end
  return proj
end

prova.test("the run's resolved selection is a readable fact in every state", {
  covers = "docs/design/verifiers.md#selection-pushdown-into-conducts",
  proves = "pushdown needs no callback protocol — the deputy's package owns its framework's filter grammar, so the engine's whole contribution is the axes as data, every axis present (possibly empty) so consumers index without nil-guards",
}, function(t)
  local proj = package(t, { ["axes_test.lua"] = [[
prova.test("alpha reads the axes", function(t)
  print("SEL " .. json.encode({
    keywords = prova.selection.keywords,
    tag_excludes = prova.selection.tag_excludes,
    is_empty = prova.selection.is_empty,
  }))
  t:expect(true):is_true()
end)
]] })
  local r = shell.run(prova.bin .. " -k alpha --tags '!flaky'", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the keyword axis travels"):contains('"keywords":["alpha"]')
  t:expect(r.stdout, "the exclusion axes travel too"):contains('"tag_excludes":["flaky"]')
  t:expect(r.stdout):contains('"is_empty":false')

  local bare = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(bare.stdout, "an unselected run says so"):contains('"is_empty":true')
end)

prova.test("a run-scoped deputy's factory sees the selection and can narrow its conduct", {
  covers = "docs/design/verifiers.md#selection-pushdown-into-conducts",
  proves = "`prova -k seed_memory` compiling to one filtered conduct under the same locks and adoption is the whole development-ladder promise — and it is package code, not engine magic: the factory reads the axes at conduct time and shapes its own command",
}, function(t)
  local proj = package(t, { ["readers_test.lua"] = [[
local deputy = prova.fixture("seen-selection", Scope.Run, function()
  fs.write(os.getenv("SEEN"), json.encode(prova.selection.keywords))
  return "conducted"
end)
prova.test("alpha reader", function(t) t:expect(t:use(deputy)):equals("conducted") end)
prova.test("beta reader", function(t) t:expect(t:use(deputy)):equals("conducted") end)
]] })
  local seen = proj .. "/seen.json"
  local r = shell.run(prova.bin .. " -k alpha", { cwd = proj, merge_stderr = true, env = { SEEN = seen } })
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the selection narrowed the proofs"):contains("1 passed")
  t:expect(fs.read(seen), "…and the deputy conducted knowing exactly why"):equals('["alpha"]')
end)

prova.test("a narrowed run's deputed account says NARROWED wherever it is read", {
  covers = "docs/design/verifiers.md#selection-pushdown-into-conducts",
  proves = "partial must never wear full's face: the account is honest for what it lists and silent about what the narrowing excluded, so attest of an absent case names the narrowing instead of implying the case never existed — and the unnarrowed CI shape records clean",
}, function(t)
  local proj = package(t, { ["adopt_test.lua"] = [[
prova.test("alpha adopts the deputy report", function(t)
  junit.verify(t, { results = os.getenv("JUNIT_PATH") })
end)
]] })
  local artifact = proj .. "/deputy.xml"
  fs.write(artifact, '<testsuites><testsuite name="S"><testcase classname="S" name="present_case" time="0.01"/></testsuite></testsuites>')
  local env = { JUNIT_PATH = artifact }

  -- Narrowed: the adopting proof is selected by keyword, so the record marks the account.
  local r = shell.run(prova.bin .. " -k alpha", { cwd = proj, merge_stderr = true, env = env })
  t:expect(r.code):equals(0)
  local attest = shell.run(prova.bin .. " attest junit:S#not_conducted", { cwd = proj, merge_stderr = true })
  t:expect(attest.code, "absent attests nothing"):never():equals(0)
  t:expect(attest.stdout, "…and the narrowing is named"):contains("NARROWED")
  local evidence = shell.run(prova.bin .. " evidence", { cwd = proj, merge_stderr = true })
  t:expect(evidence.stdout, "the account reads as partial"):contains("NARROWED run")

  -- Unnarrowed: the same absence is reported without blaming a narrowing that never happened.
  shell.run(prova.bin, { cwd = proj, merge_stderr = true, env = env })
  local clean = shell.run(prova.bin .. " attest junit:S#not_conducted", { cwd = proj, merge_stderr = true })
  t:expect(clean.code):never():equals(0)
  t:expect(clean.stdout, "no narrowing, no excuse"):never():contains("NARROWED")
end)
