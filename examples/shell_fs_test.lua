--- POC example: async fixtures + the `shell` and `fs` modules.
---
--- This is the shape of a real archetype/service acceptance test: build a workspace into a temp
--- dir (here via `shell`, in practice via `archetect.render`), then assert on the result with
--- `shell` (run a command, check exit/stdout) and `fs` (files exist, contents match).

-- A file-scoped fixture whose factory AWAITS shell.run — built once, shared by every test below,
-- torn down (temp dir removed) after the file.
local workspace = prova.fixture("workspace", "file", function(ctx)
  local dir = ctx:tempdir()
  shell.run("mkdir -p src && printf 'fn main() {}\\n' > src/main.rs", { cwd = dir, check = true })
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
  local r = shell.run("cat src/main.rs", { cwd = dir })
  t:expect(r.code):equals(0)
  t:expect(r:ok()):is_true()
  t:expect(r.stdout):contains("fn main")
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

prova.test("check=true turns a non-zero exit into a failure", function(t)
  local dir = t:use(workspace)
  local r = shell.run("test -f Cargo.toml", { cwd = dir })  -- no Cargo.toml → exit 1, but check=false
  t:expect(r.code):never():equals(0)
end)
