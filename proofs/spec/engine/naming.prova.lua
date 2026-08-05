-- File naming: `<stem>.prova.lua` is the preferred declaration-file spelling — named for the
-- collector, not one role, because a file may declare tests, fixtures, topologies, and reminders.
-- The original `_test.lua` / `.test.lua` are accepted quietly (and possibly indefinitely); bare
-- `prova.lua` is the manifest's companion file and is never collected.
--
-- THIS FILE is the first half of the evidence: it uses the new suffix, so its own execution
-- proves the collector recognizes it. The sandbox below proves the rest of the matrix.

local sandbox = prova.fixture("naming-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  -- One test per spelling, each naming its spelling — plus a bare prova.lua that must NOT load
  -- (it would raise if collected).
  fs.write(proj .. "/proofs/orders.prova.lua", [[
prova.test("dotted prova spelling collects", function(t) t:expect(true):is_true() end)
]])
  fs.write(proj .. "/proofs/queue_prova.lua", [[
prova.test("underscore prova spelling collects", function(t) t:expect(true):is_true() end)
]])
  fs.write(proj .. "/proofs/legacy_test.lua", [[
prova.test("underscore test spelling still collects", function(t) t:expect(true):is_true() end)
]])
  fs.write(proj .. "/proofs/older.test.lua", [[
prova.test("dotted test spelling still collects", function(t) t:expect(true):is_true() end)
]])
  fs.write(proj .. "/proofs/prova.lua", [[
error("bare prova.lua is the companion's name — collecting it would be the ambiguity the rule forbids")
]])
  return proj
end)

prova.test("all four spellings collect; bare prova.lua never does", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("4 passed")
  t:expect(r.stdout):contains("dotted prova spelling collects")
  t:expect(r.stdout):contains("underscore prova spelling collects")
  t:expect(r.stdout):contains("underscore test spelling still collects")
  t:expect(r.stdout):contains("dotted test spelling still collects")
end)

prova.test("the accepted spellings are quiet — no deprecation nag, ever", function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  -- The retirement question is deferred to 1.0; until then a fleet of _test.lua suites must run
  -- without a word said about their names — nagging on every run of every existing suite is how
  -- stderr gets ignored.
  t:expect(r.stdout):never():contains("deprecat")
  t:expect(r.stdout):never():contains("rename")
end)

prova.test("the binary teaches the preferred spelling", function(t)
  local r = shell.run(prova.bin .. " learn authoring", { merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains(".prova.lua")
end)
