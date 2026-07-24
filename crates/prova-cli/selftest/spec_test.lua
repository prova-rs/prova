--- The `spec` flag through the real binary (docs/plans/api-freeze.md §5): open specs keep CI
--- green but are visibly counted; an honored spec fails until graduated; `--specs` selects the
--- spec surface; `--strict-specs` is the implementing agent's driver mode; TAP renders open
--- specs as `# TODO`.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")

local function run(args)
  return shell.run(prova_bin .. " " .. args)
end

-- One temp suite reused across cases: an open spec, an honored-but-unflagged spec is exercised
-- separately (it fails the run), a graduated test, and an ordinary test.
local function write_suite(body)
  local dir = fs.tempdir()
  fs.write(dir .. "/spec_fixture_test.lua", body)
  return dir
end

local open_suite = write_suite(
  'prova.group("formats", { spec = "api-freeze" }, function(g)\n' ..
  '  g:test("json round-trips", function(t) t:expect(1):equals(2) end)\n' ..
  '  g:test("yaml parses", { spec = false }, function(t) t:expect(true):is_true() end)\n' ..
  'end)\n' ..
  'prova.test("ordinary", function(t) t:expect(1):equals(1) end)\n')

prova.test("open specs keep the run green and are counted", function(t)
  local r = run(open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("SPEC")
  t:expect(r.stdout):contains("1 spec open")
  t:expect(r.stdout):contains("(1 graduated)")
end)

prova.test("an honored spec fails demanding graduation", function(t)
  local dir = write_suite(
    'prova.test("done already", { spec = "oops" }, function(t) t:expect(1):equals(1) end)\n')
  local r = run(dir)
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("spec honored")
  t:expect(r.stdout):contains("graduate")
end)

prova.test("--strict-specs turns open specs into failures", function(t)
  local r = run("--strict-specs " .. open_suite)
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("1 failed")
end)

prova.test("--specs selects only the spec surface", function(t)
  local r = run("--specs " .. open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 spec open")
  -- the graduated and ordinary tests are deselected, not run
  t:expect(r.stdout):contains("deselected")
  t:expect(r.stdout:find("PASS", 1, true)):is_falsy()
end)

prova.test("--specs --list enumerates the open surface without running", function(t)
  local r = run("--specs --list " .. open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("json round-trips")
  t:expect(r.stdout:find("ordinary", 1, true)):is_falsy()
end)

prova.test("TAP renders an open spec as a TODO directive", function(t)
  local r = run("--format tap " .. open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("# TODO api-freeze")
end)

prova.test("a fully-graduated flag is a completion error", function(t)
  local dir = write_suite(
    'prova.group("done", { spec = "shipped" }, function(g)\n' ..
    '  g:test("a", { spec = false }, function(t) t:expect(1):equals(1) end)\n' ..
    'end)\n')
  local r = run(dir)
  t:expect(r.code):never():equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out):contains("spec complete")
  t:expect(out):contains("remove the flag")
end)
