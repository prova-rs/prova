--- "Not now": a run whose build horai refused admission has no verdict.
---
--- `horai run` exits 75 (EX_TEMPFAIL) when the disk budget can't admit a build. Whatever fails after
--- that says nothing about the change, so the run must not be red: it ends 75 and its record's verdict
--- is `not-now`, which a gate (Herald) reads as "try again later", never a bounce. Only a refusal from
--- the horai that WRAPPED the build counts; a command of the proof's own that happens to end 75 is an
--- ordinary failure, or anything could launder red into "not now".

local function project(ctx, body)
  local root = ctx:tempdir()
  local bin = root .. "/bin"
  fs.mkdir(bin)
  -- A horai that counts sessions (its usage names HORAI_SESSION) and refuses every admission.
  fs.write(bin .. "/horai", [[#!/bin/sh
if [ "$1" = run ] && [ $# -eq 1 ]; then
  echo "run: usage: horai run -- <command> (HORAI_SESSION)" >&2
  exit 2
fi
echo "horai: admission refused: the build dir is over budget" >&2
exit 75
]])
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
]])
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
