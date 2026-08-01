--- fs.mkdir(path) — the platform-agnostic replacement for shell.run("mkdir -p ..."), which routes
--- through `cmd /C` on Windows where `-p` is not a flag and `/` is not a separator. Creates every
--- missing parent (like `mkdir -p`) and is idempotent. Runs everywhere; no shell.

prova.test("fs.mkdir creates the directory and all missing parents",
  { proves = "increment-2: fs.mkdir creates parents like mkdir -p" }, function(t)
  local deep = t:tempdir() .. "/a/b/c"
  fs.mkdir(deep)
  t:expect(fs.exists(deep)):is_true()
  -- a real, empty directory: writing a file under it works
  fs.write(deep .. "/x.txt", "hi")
  t:expect(fs.read(deep .. "/x.txt")):equals("hi")
end)

prova.test("fs.mkdir is idempotent — a second call on an existing dir is a no-op",
  { proves = "increment-2: fs.mkdir is idempotent" }, function(t)
  local d = t:tempdir() .. "/dir"
  fs.mkdir(d)
  fs.mkdir(d)                       -- no error the second time
  t:expect(fs.exists(d)):is_true()
end)
