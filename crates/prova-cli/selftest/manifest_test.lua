--- Prova testing Prova: acceptance-test the suite manifest by writing a `prova.toml` into a temp
--- dir, invoking the real binary there, and asserting the profile selection + env injection.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")

-- A scratch project: a manifest with two profiles pointing at different files, plus an env var the
-- test file reads back.
local function project()
  local dir = fs.tempdir()
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]',
    'proofs = ["green"]',            -- default profile discovers the green/ proofs dir
    '[run.env]',
    'PROVA_SELFTEST_ENV = "from-manifest"',
    '',
    '[profiles.red]',
    'proofs = ["red"]',             -- --profile red discovers the red/ proofs dir instead
  }, "\n"))
  fs.write(dir .. "/green/env_test.lua",
    'prova.test("env is injected", function(t) t:expect(os.getenv("PROVA_SELFTEST_ENV")):equals("from-manifest") end)')
  fs.write(dir .. "/red/fail_test.lua",
    'prova.test("fails on purpose", function(t) t:expect(1):equals(2) end)')
  return dir
end

prova.test("bare `prova` runs the default profile and injects env", function(t)
  local dir = project()
  local r = shell.run(prova_bin, { cwd = dir })   -- no args → reads ./prova.toml
  t:expect(r.code):equals(0)                        -- green.lua passes (env was injected)
  t:expect(r.stdout):contains("env is injected")
end)

prova.test("--profile selects the profile's paths", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " --profile red", { cwd = dir })
  t:expect(r.code):equals(1)                        -- red.lua fails
  t:expect(r.stdout):contains("fails on purpose")
end)

prova.test("an unknown profile is a usage error (exit 2)", function(t)
  local dir = project()
  local r = shell.run(prova_bin .. " --profile nope", { cwd = dir })
  t:expect(r.code):equals(2)
  t:expect(r.stderr):contains("no such profile")
end)
