-- Dogfoods the shell and fs modules: build a workspace in a temp dir with fs, then probe it with
-- shell (run a command, check exit/stdout) and fs (files exist, contents match) — plus a
-- Scope.File fixture built once and torn down after the file. Every command is portable: the one
-- program every platform running this suite is guaranteed to have is prova itself (prova.bin).

local workspace = prova.fixture("workspace", Scope.File, function(ctx)
  local dir = ctx:tempdir()
  fs.write(dir .. "/src/main.rs", "fn main() {}\n")   -- fs.write creates the parent dirs
  return dir
end)

prova.test("the workspace has the rendered source file", function(t)
  local dir = t:use(workspace)
  t:expect(fs.exists(dir .. "/src/main.rs")):is_true()   -- fs.exists
  t:expect(dir .. "/src/main.rs"):exists()               -- filesystem matcher on a path string
  t:expect(dir .. "/src"):is_dir()
end)

prova.test("shell.run reports exit code and stdout", function(t)
  local dir = t:use(workspace)
  local r = shell.run(prova.bin .. " --version", { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r:ok()):is_true()
  t:expect(r.stdout):contains("prova")
end)

prova.test("fs.read returns the file contents", function(t)
  local dir = t:use(workspace)                            -- same workspace instance (file scope)
  t:expect(fs.read(dir .. "/src/main.rs")):contains("fn main")
end)

prova.test("fs.glob finds the source tree", function(t)
  local dir = t:use(workspace)
  local hits = fs.glob(dir, "**/*.rs")
  t:expect(#hits):equals(1)
  t:expect(hits[1]):contains("main.rs")
end)

prova.test("a non-zero exit is reported, not raised, without check", function(t)
  local dir = t:use(workspace)
  -- argv form: no shell anywhere, so the same spelling exits non-zero on every OS
  local r = shell.run({ prova.bin, "--no-such-flag" }, { cwd = dir, merge_stderr = true })
  t:expect(r.code):never():equals(0)
end)
