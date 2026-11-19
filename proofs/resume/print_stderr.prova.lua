--- Under a machine report (`--format json`, `tap`) a suite's own `print` goes to stderr, never into it (E, 2026-11-19): json is one JSON event per
--- line on stdout, and a reader that parses it strictly (Horai's bounce sorting) refused substrate's gate verdict as
--- unreadable because substrate's package printed "workspace conduct: …" into it. stdout is the report's alone.

local sandbox = prova.fixture("print-stderr-sandbox", Scope.Test, function(ctx)
  local pkg = ctx:tempdir() .. "/pkg"
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/chatty_test.lua", [[
print("chatter at load", 1, true)
io.write("written at load\n")
io.stdout:write("stdout handle at load\n")
prova.test("a test that prints", function(t)
  print("chatter inside a test")
  t:expect(1):equals(1)
end)
]])
  return { pkg = pkg }
end)

prova.test("with --format json every stdout line is JSON, and a suite's print reaches stderr", function(t)
  local sb = t:use(sandbox)
  local r = shell.run(prova.bin .. " --format json", { cwd = sb.pkg, env = { PROVA_VAR_DIR = "" } })
  t:expect(r.code, r.stderr):equals(0)
  for line in r.stdout:gmatch("[^\n]+") do
    local ok = pcall(json.decode, line)
    t:expect(ok, "a stdout line that isn't JSON: " .. line):is_true()
  end
  t:expect(r.stderr):contains("chatter at load\t1\ttrue")
  t:expect(r.stderr):contains("chatter inside a test")
  t:expect(r.stderr):contains("written at load")
  t:expect(r.stderr):contains("stdout handle at load")
end)

prova.test("with --format json a suite's os.execute and io.popen(_, \"w\") are refused by name, never writing into the report", function(t)
  local pkg = t:tempdir() .. "/pkg"
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/child_test.lua", [[
prova.test("execute", function(t) os.execute("echo from-execute") end)
prova.test("popen w", function(t) io.popen("cat", "w"):write("from-popen\n") end)
prova.test("popen r still reads", function(t) t:expect(io.popen("echo hi", "r"):read("l")):equals("hi") end)
]])
  local r = shell.run(prova.bin .. " --format json", { cwd = pkg, env = { PROVA_VAR_DIR = "" } })
  for line in r.stdout:gmatch("[^\n]+") do
    t:expect(pcall(json.decode, line), "a stdout line that isn't JSON: " .. line):is_true()
  end
  t:expect(r.stdout):contains("use prova's shell module")
  t:expect(r.stdout:find("from-execute", 1, true)):is_nil()
end)

prova.test("on the console a suite's print stays on stdout, in order with its PASS lines", function(t)
  local sb = t:use(sandbox)
  local r = shell.run(prova.bin, { cwd = sb.pkg, env = { PROVA_VAR_DIR = "" } })
  t:expect(r.code, r.stderr):equals(0)
  t:expect(r.stdout):contains("chatter inside a test")
end)
