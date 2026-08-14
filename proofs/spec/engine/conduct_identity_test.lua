--- Conduct identity (docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts,
--- docs/plans/incremental-prova.md increment 1): two conducts that depend on the same things are
--- one execution, and `fs.digest` is the primitive that says so.
---
--- The measured motivation: 93% of a `run all` on this tree is four cargo conducts. Within a run,
--- `Scope.Run` already conducts each NAME once — what it cannot see is that two differently-named
--- deputies are asking the same question of the same tool.

local scaffold = require("scaffold")

--- `fs.digest` is new in the tree, so it must be reached through the SUBJECT: called in this body
--- it would exercise whichever prova is conducting the suite, where the field does not exist yet.
local function subject_eval(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "30s" })
end

prova.test("fs.digest answers by content, not by path or order", {
  covers = "docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts",
  proves = "an identity that moved when a listing order or an argument order moved would dedupe nothing while claiming to — it has to be a fact about the bytes, or the mechanism degrades to 'always re-run' silently",
}, function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/a.txt", "alpha\n")
  fs.write(dir .. "/b.txt", "beta\n")

  local r = subject_eval(string.format([[
local d = "%s"
local both = fs.digest({ d .. "/a.txt", d .. "/b.txt" })
print("both=" .. both)
print("reversed=" .. fs.digest({ d .. "/b.txt", d .. "/a.txt" }))
print("globbed=" .. fs.digest(d .. "/*.txt"))
fs.write(d .. "/b.txt", "beta changed\n")
print("after=" .. fs.digest({ d .. "/a.txt", d .. "/b.txt" }))
]], dir))
  local both = r.stdout:match("both=(%x+)")
  t:expect(both, "a digest is lowercase hex:\n" .. r.stdout):is_truthy()
  t:expect(r.stdout:match("reversed=(%x+)"), "argument order is not part of the answer"):equals(both)
  t:expect(r.stdout:match("globbed=(%x+)"), "a glob resolves to the same set"):equals(both)
  t:expect(r.stdout:match("after=(%x+)"), "content moves it"):never():equals(both)
end)

prova.test("an absent path is part of the answer, not an error", {
  covers = "docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts",
  proves = "raising on a missing input would make identity unusable for the case it exists for — a generated input that is not there yet — and treating absence as equal to presence would replay a stale answer straight across the file appearing",
}, function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/there.txt", "x\n")

  local r = subject_eval(string.format([[
local d = "%s"
print("without=" .. fs.digest({ d .. "/there.txt", d .. "/missing.txt" }))
fs.write(d .. "/missing.txt", "now here\n")
print("with=" .. fs.digest({ d .. "/there.txt", d .. "/missing.txt" }))
]], dir))
  local without = r.stdout:match("without=(%x+)")
  t:expect(without, "a missing path digests rather than raising:\n" .. r.stdout):is_truthy()
  t:expect(r.stdout:match("with=(%x+)"), "…and its arrival moves the digest"):never():equals(without)
end)

prova.test("two differently-named fixtures with one identity conduct once", {
  covers = "docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts",
  proves = "the field shape: two proof files each own a deputy conducting the same cargo with the same packages and profile, and every sweep pays ~100-140s twice for two junit copies differing only in filename. Names are the isolation boundary and must stay; the EXECUTION behind them is what should be shared",
}, function(t)
  local proj = scaffold.package(t, { proofs = {} })
  fs.mkdir(proj .. "/src")
  fs.write(proj .. "/src/thing.txt", "the input\n")
  local counter = proj .. "/conducts.log"

  -- Two suites, two fixture NAMES, one identity: whichever runs first conducts.
  local body = [[
local slow = prova.fixture("NAME", Scope.Run, function()
  shell.run({ "sh", "-c", 'printf "x\n" >> "$1"', "sh", "]] .. counter .. [[" })
  return "conducted"
end, { identity = { command = "the-shared-conduct", inputs = { "src/thing.txt" } } })

prova.test("reads it", function(t)
  t:expect(t:use(slow)):equals("conducted")
end)
]]
  fs.mkdir(proj .. "/proofs/a")
  fs.mkdir(proj .. "/proofs/b")
  fs.write(proj .. "/proofs/a/one_test.lua", body:gsub("NAME", "deputy-one"))
  fs.write(proj .. "/proofs/b/two_test.lua", body:gsub("NAME", "deputy-two"))

  local r = shell.run({ prova.bin, "-j", "2" }, { cwd = proj, merge_stderr = true, timeout = "120s" })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("2 passed")

  local conducts = 0
  for _ in fs.read(counter):gmatch("x") do conducts = conducts + 1 end
  t:expect(conducts, "one execution served both names"):equals(1)
end)

prova.test("different identities still conduct separately — sharing is by fact, not by hope", {
  covers = "docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts",
  proves = "the negative control that makes the sharing safe: two deputies over different inputs answer different questions, and a store that collapsed them would hand one tool's verdict to the other's readers — a far worse failure than paying twice",
}, function(t)
  local proj = scaffold.package(t, { proofs = {} })
  fs.mkdir(proj .. "/src")
  fs.write(proj .. "/src/one.txt", "first\n")
  fs.write(proj .. "/src/two.txt", "second\n")
  local counter = proj .. "/conducts.log"

  local body = [[
local slow = prova.fixture("NAME", Scope.Run, function()
  shell.run({ "sh", "-c", 'printf "x\n" >> "$1"', "sh", "]] .. counter .. [[" })
  return "conducted"
end, { identity = { command = "the-shared-conduct", inputs = { "src/INPUT" } } })

prova.test("reads it", function(t)
  t:expect(t:use(slow)):equals("conducted")
end)
]]
  fs.mkdir(proj .. "/proofs/a")
  fs.mkdir(proj .. "/proofs/b")
  fs.write(proj .. "/proofs/a/one_test.lua", body:gsub("NAME", "deputy-one"):gsub("INPUT", "one.txt"))
  fs.write(proj .. "/proofs/b/two_test.lua", body:gsub("NAME", "deputy-two"):gsub("INPUT", "two.txt"))

  local r = shell.run({ prova.bin, "-j", "2" }, { cwd = proj, merge_stderr = true, timeout = "120s" })
  t:expect(r.code, r.stdout):equals(0)

  local conducts = 0
  for _ in fs.read(counter):gmatch("x") do conducts = conducts + 1 end
  t:expect(conducts, "two questions, two executions"):equals(2)
end)

prova.test("a fixture that declares no identity keeps today's semantics exactly", {
  covers = "docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts",
  proves = "silence must mean the old behavior: every Scope.Run fixture already in the wild declares nothing, and an opt-in that changed their sharing would collapse two deliberately-separate conducts the day it shipped",
}, function(t)
  local proj = scaffold.package(t, { proofs = {} })
  local counter = proj .. "/conducts.log"
  local body = [[
local slow = prova.fixture("NAME", Scope.Run, function()
  shell.run({ "sh", "-c", 'printf "x\n" >> "$1"', "sh", "]] .. counter .. [[" })
  return "conducted"
end)

prova.test("reads it", function(t)
  t:expect(t:use(slow)):equals("conducted")
end)
]]
  fs.mkdir(proj .. "/proofs/a")
  fs.mkdir(proj .. "/proofs/b")
  fs.write(proj .. "/proofs/a/one_test.lua", body:gsub("NAME", "deputy-one"))
  fs.write(proj .. "/proofs/b/two_test.lua", body:gsub("NAME", "deputy-two"))

  local r = shell.run({ prova.bin, "-j", "2" }, { cwd = proj, merge_stderr = true, timeout = "120s" })
  t:expect(r.code, r.stdout):equals(0)

  local conducts = 0
  for _ in fs.read(counter):gmatch("x") do conducts = conducts + 1 end
  t:expect(conducts, "two undeclared names stay two conducts"):equals(2)
end)
