--- Prova testing Prova: acceptance-test the `prova` CLI by invoking the real binary against inner
--- fixtures and asserting on exit codes and output. The launcher (tests/selftest.rs) sets
--- `PROVA_BIN` (the built binary) and `PROVA_FIXTURES` (this dir's fixtures).

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")
local fixtures = assert(os.getenv("PROVA_FIXTURES"), "PROVA_FIXTURES not set")

local function run(args)
  return shell.run(prova_bin .. " " .. args)
end

prova.test("a passing suite exits 0 and reports the tally", function(t)
  local r = run(fixtures .. "/passing.lua")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("2 passed")
  t:expect(r.stdout):contains("0 failed")
end)

prova.test("a suite with a failure exits 1", function(t)
  local r = run(fixtures .. "/mixed.lua")
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("1 passed")
  t:expect(r.stdout):contains("1 failed")
  t:expect(r.stdout):contains("1 skipped")
end)

prova.test("--list discovers tests without running them", function(t)
  local r = run("--list " .. fixtures .. "/passing.lua")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("adds numbers")
  t:expect(r.stdout):contains("compares strings")
end)

prova.test("--format json emits the JSONL event protocol", function(t)
  local r = run("--format json " .. fixtures .. "/passing.lua")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains('"type":"node_finished"')
  t:expect(r.stdout):contains('"outcome":"passed"')
end)

prova.test("--format tap emits the TAP protocol", function(t)
  local r = run("--format tap " .. fixtures .. "/mixed.lua")
  t:expect(r.code):equals(1)                              -- mixed has a failure
  t:expect(r.stdout):contains("TAP version 13")
  t:expect(r.stdout):contains("ok ")
  t:expect(r.stdout):contains("not ok ")
  t:expect(r.stdout):contains("1..")                      -- trailing plan
end)

prova.test("--junit writes a JUnit XML file alongside console output", function(t)
  local out = fs.tempdir() .. "/results.xml"
  local r = run("--junit " .. out .. " " .. fixtures .. "/mixed.lua")
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("passed")                  -- console still prints
  t:expect(fs.exists(out)):is_truthy()                   -- and the file is written
  local xml = fs.read(out)
  t:expect(xml):contains("<testsuites")
  t:expect(xml):contains('failures="1"')
  t:expect(xml):contains("<failure")
end)

prova.test("snapshots: update writes, re-run matches, a change fails with a diff", function(t)
  local dir = fs.tempdir()
  local test = dir .. "/snap_test.lua"
  local function write_value(v)
    fs.write(test, 'prova.test("greeting", function(t) t:expect("' .. v .. '"):matches_snapshot() end)\n')
  end

  -- Missing snapshot without --update-snapshots fails.
  write_value("hello")
  t:expect(run(test).code):equals(1)

  -- --update-snapshots writes it and passes; the .snap lands beside the test file.
  local upd = run("-u " .. test)
  t:expect(upd.code):equals(0)
  t:expect(fs.exists(dir .. "/snapshots")):is_truthy()

  -- A clean re-run matches.
  t:expect(run(test).code):equals(0)

  -- Changing the value fails with a mismatch + diff.
  write_value("goodbye")
  local changed = run(test)
  t:expect(changed.code):equals(1)
  t:expect(changed.stdout):contains("snapshot mismatch")
  t:expect(changed.stdout):contains("- hello")
  t:expect(changed.stdout):contains("+ goodbye")
end)

prova.test("an unknown flag is a usage error (exit 2)", function(t)
  local r = run("--definitely-not-a-flag")
  t:expect(r.code):equals(2)
end)

prova.test("no test files found is an error (exit 2)", function(t)
  local empty = fs.tempdir()
  local r = run(empty)
  t:expect(r.code):equals(2)
  t:expect(r.stderr):contains("no test files")
end)
