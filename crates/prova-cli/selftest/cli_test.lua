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
  -- Failures are findable: each line carries its source location, and a recap re-states every
  -- failure at the end with a copy-pasteable exact-node rerun.
  t:expect(r.stdout):contains("failures:")
  t:expect(r.stdout):contains("mixed.lua:")
  t:expect(r.stdout):contains("rerun: prova --node")
end)

prova.test("console output is plain when piped, ANSI under --color always", function(t)
  local esc = string.char(27) .. "["
  -- Captured output is a pipe, so the default (auto) emits no escape codes...
  local piped = run(fixtures .. "/passing.lua")
  t:expect(piped.stdout:find(esc, 1, true)):is_falsy()
  -- ...and `--color always` forces them even into a pipe (what CI log viewers want).
  local forced = run("--color always " .. fixtures .. "/passing.lua")
  t:expect(forced.stdout:find(esc, 1, true)):is_truthy()
end)

prova.test("console output is a tree: file header, group header, indented leaf", function(t)
  local dir = fs.tempdir()
  fs.write(dir .. "/hier_test.lua",
    'prova.group("billing", function(g)\n' ..
    '  g:test("refund", function(t) t:expect(true):is_truthy() end)\n' ..
    'end)\n')
  local r = run(dir .. "/hier_test.lua")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("hier_test.lua")           -- the file section header
  t:expect(r.stdout):contains("\n  billing\n")           -- the group header, indented under it
  t:expect(r.stdout):contains("    PASS  refund")        -- the leaf, indented under the group
  -- The leaf prints its own name only — the ancestry lives in the headers above it.
  t:expect(r.stdout:find("billing › refund", 1, true)):is_falsy()
end)

prova.test("--quiet suppresses PASS/SKIP lines but keeps the tally", function(t)
  local r = run("-q " .. fixtures .. "/passing.lua")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("2 passed")
  t:expect(r.stdout:find("PASS", 1, true)):is_falsy()
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
  -- Events carry the declaration's source location (file path + line).
  t:expect(r.stdout):contains('"file":')
  t:expect(r.stdout):contains('passing.lua')
  t:expect(r.stdout):contains('"line":')
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
  -- Test cases carry their source location, so dashboards can link back to the file.
  t:expect(xml):contains('file="')
  t:expect(xml):contains('mixed.lua')
  t:expect(xml):contains('line="')
  -- Run metadata: a wall-clock timestamp, errors attr, per-case assertion counts, properties.
  t:expect(xml):contains('timestamp="')
  t:expect(xml):contains('errors="0"')
  t:expect(xml):contains('assertions="')
  t:expect(xml):contains('name="prova.version"')
end)

prova.test("a manifest junit key writes the report with the package's suite name", function(t)
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\njunit = "results.xml"\n')
  fs.write(dir .. "/proofs/one_test.lua",
    'prova.test("adds", function(t) t:expect(1+1):equals(2) end)\n')
  local r = shell.run(prova_bin, { cwd = dir })
  t:expect(r.code):equals(0)
  local xml = fs.read(dir .. "/results.xml")
  t:expect(xml):contains("<testsuites")
  -- The suite is named after the package (the home directory's basename), not a hardcoded "prova".
  local base = dir:match("([^/\\]+)$")
  t:expect(xml):contains('<testsuite name="' .. base .. '"')
end)

prova.test("GitHub Actions mode: annotations + step summary, honoring --gha off", function(t)
  local summary = fs.tempdir() .. "/step_summary.md"
  local gha_env = { GITHUB_ACTIONS = "true", GITHUB_STEP_SUMMARY = summary }

  -- Detected via GITHUB_ACTIONS=true: failures become ::error annotations (with file/line
  -- properties) alongside the normal console output, and the run appends a markdown summary.
  local r = shell.run(prova_bin .. " " .. fixtures .. "/mixed.lua", { env = gha_env })
  t:expect(r.code):equals(1)
  t:expect(r.stdout):contains("::error file=")
  t:expect(r.stdout):contains("line=")
  t:expect(r.stdout):contains("1 failed")            -- console tally still prints
  local md = fs.read(summary)
  t:expect(md):contains("prova —")
  t:expect(md):contains("| ❌ |")
  t:expect(md):contains("mixed.lua")

  -- The escape hatch: --gha off suppresses the sink even inside Actions.
  local off = shell.run(prova_bin .. " --gha off " .. fixtures .. "/mixed.lua", { env = gha_env })
  t:expect(off.stdout:find("::error", 1, true)):is_falsy()
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

prova.test("snapshots: a layout snapshot catches an added file", function(t)
  local work = fs.tempdir()
  local render = work .. "/render"
  -- No mkdir: `fs.write` creates parent directories. The `mkdir -p` this used to run was both
  -- redundant and unix-only — and, having no `check = true`, it failed SILENTLY on Windows, leaving
  -- no render dir and failing the snapshot below for a reason that pointed nowhere near the cause.
  fs.write(render .. "/Cargo.toml", "[package]")
  fs.write(render .. "/src/main.rs", "fn main() {}")
  -- The test file lives in `work` so its .snap lands in work/snapshots.
  --
  -- The path goes in as a LONG BRACKET string, not a quoted one: this generates Lua source, and on
  -- Windows the temp path is `C:\Users\…`, whose backslashes a quoted literal would read as escape
  -- sequences (`\U` → invalid escape → the generated file does not parse, and the run below exits 1
  -- for a reason nothing about "layout snapshot" would lead you to). Long brackets take the string
  -- verbatim.
  local test = work .. "/layout_test.lua"
  fs.write(test, 'prova.test("layout", function(t) t:expect({ path = [[' .. render ..
    ']] }):matches_snapshot("shape") end)\n')

  t:expect(run("-u " .. test).code):equals(0)     -- record the layout
  t:expect(run(test).code):equals(0)              -- unchanged → matches

  fs.write(render .. "/src/extra.rs", "// oops")  -- an unexpected file appears
  local changed = run(test)
  t:expect(changed.code):equals(1)
  t:expect(changed.stdout):contains("+ src/extra.rs")
end)

prova.test("snapshots: --unreferenced flags an orphaned snapshot on a full run", function(t)
  local dir = fs.tempdir()
  local test = dir .. "/unref_test.lua"
  fs.write(test, 'prova.test("a", function(t) t:expect("x"):matches_snapshot() end)\n' ..
                 'prova.test("b", function(t) t:expect("y"):matches_snapshot() end)\n')
  t:expect(run("-u " .. test).code):equals(0)              -- write both snaps

  -- Drop test "b" → its snapshot is now orphaned.
  fs.write(test, 'prova.test("a", function(t) t:expect("x"):matches_snapshot() end)\n')
  local warned = run("--unreferenced warn " .. test)
  t:expect(warned.code):equals(1)                          -- warn fails the run so CI catches rot
  t:expect(warned.stderr):contains("unreferenced snapshot")

  -- A filtered run skips the check (soundness), so it passes.
  local filtered = run("--unreferenced warn -k a " .. test)
  t:expect(filtered.stderr):contains("skipped on a filtered run")

  -- delete removes the orphan; a subsequent full run is clean.
  t:expect(run("--unreferenced delete " .. test).code):equals(0)
  t:expect(run("--unreferenced warn " .. test).code):equals(0)
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
