--- "Not now": a run whose build horai refused admission has no verdict.
---
--- `horai run` exits 75 (EX_TEMPFAIL) when the disk budget can't admit a build. Whatever fails after
--- that says nothing about the change, so the run must not be red: it ends 75 and its record's verdict
--- is `not-now`, which a gate (Herald) reads as "try again later", never a bounce. Only a refusal from
--- the horai that WRAPPED the build counts; a command of the proof's own that happens to end 75 is an
--- ordinary failure, or anything could launder red into "not now".

--- A horai that counts sessions (its usage names HORAI_SESSION) and ends every build 75. With
--- `records`, it refuses admission and records the refusal under the session, answering
--- `horai refused <session>` as horai does; without, the 75 is the build's own, passed through.
local function project(ctx, body, records)
  local root = ctx:tempdir()
  local bin = root .. "/bin"
  fs.mkdir(bin)
  fs.mkdir(root .. "/refusals")
  fs.write(bin .. "/horai", ([[#!/bin/sh
R=%s/refusals
if [ "$1" = run ] && [ $# -eq 1 ]; then
  echo "run: usage: horai run -- <command> (HORAI_SESSION)" >&2
  exit 2
fi
if [ "$1" = refused ]; then
  test -f "$R/$2"
  exit $?
fi
if [ %s = yes ]; then
  : > "$R/$HORAI_SESSION"
  echo "horai: admission refused: the build dir is over budget" >&2
fi
exit 75
]]):format(root, records and "yes" or "no"))
  shell.run({ "chmod", "+x", bin .. "/horai" }, { check = true })
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/build_test.lua", body)
  return proj, bin
end

local function run(proj, bin)
  -- PROVA_HORAI is set to 1 explicitly: a gate running this suite may itself say PROVA_HORAI=0.
  local r = shell.run({ prova.bin }, {
    cwd = proj,
    merge_stderr = true,
    env = { PATH = bin .. ":" .. os.getenv("PATH"), PROVA_HORAI = "1" },
  })
  return r, json.decode(fs.read(proj .. "/.prova/var/last-run.json"))
end

prova.test("a build horai refused ends the run 75 with no verdict", {
  proves = "a refused admission is the environment not being ready, never a failed proof",
}, function(t)
  local proj, bin = project(t, [[
prova.test("builds", function(t)
  local r = shell.run({ "cargo", "build" }, { merge_stderr = true })
  t:expect(r.code):equals(0)
end)
]], true)
  local r, record = run(proj, bin)
  t:expect(r.code, r.stdout):equals(75)
  t:expect(r.stdout):contains("not now")
  t:expect(record.verdict):equals("not-now")
end)

prova.test("a proof's own command ending 75 is still red", {
  proves = "only the wrapping horai's refusal is 'not now'; an unwrapped exit 75 can't launder red",
}, function(t)
  local proj, bin = project(t, [[
prova.test("ends 75", function(t)
  local r = shell.run({ "sh", "-c", "exit 75" }, { merge_stderr = true })
  t:expect(r.code):equals(0)
end)
]])
  local r, record = run(proj, bin)
  t:expect(r.code, r.stdout):equals(1)
  t:expect(record.verdict):equals("red")
end)

prova.test("a wrapped build's own exit 75, with no horai refusal recorded, is red", {
  proves = "75 is ambiguous: horai run passes its child's exit through, so only horai's record of a refusal makes it 'not now' (D m/8928)",
}, function(t)
  local proj, bin = project(t, [[
prova.test("builds", function(t)
  local r = shell.run({ "cargo", "build" }, { merge_stderr = true })
  t:expect(r.code):equals(0)
end)
]], false)
  local r, record = run(proj, bin)
  t:expect(r.code, r.stdout):equals(1)
  t:expect(r.stdout):contains("exit 75 without a horai refusal")
  t:expect(record.verdict):equals("red")
end)
