--- The `promises` flag through the real binary (docs/plans/api-freeze.md §5, revised — test-
--- level only): open promises keep CI green but are visibly counted; a kept promise fails until
--- its flag graduates; `--specs` selects the promised surface; `--strict-specs` is the
--- implementing agent's driver mode; TAP renders open promises as `# TODO`.

local prova_bin = assert(prova.bin, "prova.bin not injected by the runtime")

local function run(args)
  return shell.run(prova_bin .. " " .. args)
end

local function write_suite(body)
  local dir = fs.tempdir()
  fs.write(dir .. "/spec_fixture_test.lua", body)
  return dir
end

-- One temp suite reused across cases: an open promise + an ordinary test.
local open_suite = write_suite(
  'prova.test("json round-trips", { promises = "api-freeze" }, function(t) t:expect(1):equals(2) end)\n' ..
  'prova.test("ordinary", function(t) t:expect(1):equals(1) end)\n')

prova.test("open promises keep the run green and are counted", function(t)
  local r = run(open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("PROMISED")
  t:expect(r.stdout):contains("1 promised")
end)

prova.test("an honored spec fails demanding graduation — convert to proves, or remove", function(t)
  local dir = write_suite(
    'prova.test("done already", { promises = "oops" }, function(t) t:expect(1):equals(1) end)\n')
  local r = run(dir)
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("promise kept")
  -- the fix is copy-pasteable: the promise's reason carried over as the proves context
  t:expect(r.stdout):contains('change `promises` to proves = "oops"')
end)

prova.test("--due turns open promises into failures", function(t)
  local r = run("--due " .. open_suite)
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("1 failed")
end)

prova.test("--promises selects only the promised surface", function(t)
  local r = run("--promises " .. open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 promised")
  -- the ordinary test is deselected, not run
  t:expect(r.stdout):contains("deselected")
  t:expect(r.stdout:find("PASS", 1, true)):is_falsy()
end)

prova.test("--promises --list enumerates the open surface without running", function(t)
  local r = run("--promises --list " .. open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("json round-trips")
  t:expect(r.stdout:find("ordinary", 1, true)):is_falsy()
end)

prova.test("TAP renders an open promise as a TODO directive", function(t)
  local r = run("--format tap " .. open_suite)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("# TODO api-freeze")
end)

prova.test("an open promise renders reason + first error line, without the traceback", function(t)
  local dir = write_suite(
    'prova.test("todo", { promises = "gap-7" }, function(t) error("json.encode is not built") end)\n')
  local r = run(dir)
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("PROMISED")
  t:expect(r.stdout):contains("gap-7")
  -- The first line of the error is the call to action…
  t:expect(r.stdout):contains("json.encode is not built")
  -- …but an EXPECTED failure carries no traceback noise (that is for unexpected red).
  t:expect(r.stdout:find("stack traceback", 1, true)):is_falsy()
end)

prova.test("--due keeps the full failure detail, traceback included", function(t)
  local dir = write_suite(
    'prova.test("todo", { promises = "gap-7" }, function(t) error("json.encode is not built") end)\n')
  local r = run("--due " .. dir)
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("stack traceback")
end)

prova.test("a group-level promises flag is refused with the fix", function(t)
  local dir = write_suite(
    'prova.group("g", { promises = "wip" }, function(g)\n' ..
    '  g:test("open", function(t) t:expect(1):equals(2) end)\n' ..
    'end)\n')
  local r = run(dir)
  t:expect(r.code):never():equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out):contains("promises is test-level only")
end)

prova.test("a bare promises flag is refused — the reason is mandatory", function(t)
  local dir = write_suite(
    'prova.test("wordless", { promises = true }, function(t) t:expect(1):equals(2) end)\n')
  local r = run(dir)
  t:expect(r.code):never():equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out):contains("reason")
end)

prova.test("promises = false is refused — an unflagged test is already a proof", function(t)
  local dir = write_suite(
    'prova.test("done", { promises = false }, function(t) t:expect(1):equals(1) end)\n')
  local r = run(dir)
  t:expect(r.code):never():equals(0)
  local out = r.stdout .. r.stderr
  t:expect(out):contains("remove the entry")
end)
