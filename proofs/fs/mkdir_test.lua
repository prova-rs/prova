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

prova.test("fs.glob returns a list even when nothing matches", {
  proves = "a glob that matched nothing used to encode as `{}` rather than `[]`, so a proof that sent its result onward changed the request's shape whenever the directory happened to be empty — the failure only shows up on the empty path, which is exactly the case least likely to be exercised while writing the proof",
}, function(t)
  local dir = t:tempdir("globbing")
  t:expect(json.encode(fs.glob(dir, "**/*.nothing")), "no matches is an empty LIST"):equals("[]")

  fs.write(dir .. "/one.txt", "x")
  local hits = fs.glob(dir, "**/*.txt")
  t:expect(#hits, "matches are unaffected"):equals(1)
  t:expect(json.encode(hits):sub(1, 1), "…and still encode as a list"):equals("[")
end)
