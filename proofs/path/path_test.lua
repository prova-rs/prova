--- prova.path — pure, platform-agnostic path algebra. String-based on purpose (NOT std::path,
--- which prints `\` on Windows): every function accepts either separator and emits `/`-normalized
--- strings, so the SAME assertions hold on every OS. No filesystem access — these are string laws.
--- Canonical access is `prova.path`; the bare name `path` is too collision-prone to squat, so it is
--- ambient only when a package asks for it via `[globals] inject`.

local p = prova.path

prova.test("path.join joins segments with '/', absorbing extra separators",
  { proves = "increment-3: prova.path.join" }, function(t)
  t:expect(p.join("a", "b", "c")):equals("a/b/c")
  t:expect(p.join("a/", "b")):equals("a/b")
  t:expect(p.join("a\\b", "c")):equals("a/b/c")     -- backslashes normalize on the way in
  t:expect(p.join("a", "", "b")):equals("a/b")      -- empty segments contribute nothing
  t:expect(p.join("/etc", "app")):equals("/etc/app")
  t:expect(p.join("C:/Users", "x")):equals("C:/Users/x")
  -- an absolute later segment resets the join (std::path law — predictable, not surprising)
  t:expect(p.join("a", "/etc", "b")):equals("/etc/b")
end)

prova.test("path.dirname is everything before the last component ('.' for a bare name)",
  { proves = "increment-3: prova.path.dirname" }, function(t)
  t:expect(p.dirname("a/b/c.txt")):equals("a/b")
  t:expect(p.dirname("c.txt")):equals(".")          -- bare name: the current dir
  t:expect(p.dirname("a/b/")):equals("a")           -- trailing slash does not make a component
  t:expect(p.dirname("/a")):equals("/")
  t:expect(p.dirname("/")):equals("/")
  t:expect(p.dirname("C:\\Users\\x")):equals("C:/Users")
  t:expect(p.dirname("C:/a")):equals("C:/")
end)

prova.test("path.basename is the last component",
  { proves = "increment-3: prova.path.basename" }, function(t)
  t:expect(p.basename("a/b/c.txt")):equals("c.txt")
  t:expect(p.basename("c.txt")):equals("c.txt")
  t:expect(p.basename("a/b/")):equals("b")
  t:expect(p.basename("C:\\Users\\x.rs")):equals("x.rs")
  t:expect(p.basename("/")):equals("")              -- a root has no last component
end)

prova.test("path.ext is the extension without the dot; path.stem is the basename without it",
  { proves = "increment-3: prova.path.ext + stem" }, function(t)
  t:expect(p.ext("a/b.txt")):equals("txt")          -- no dot, by design ("txt", not ".txt")
  t:expect(p.ext("b.tar.gz")):equals("gz")
  t:expect(p.ext("a/b")):equals("")                 -- no extension: empty string, not nil
  t:expect(p.ext(".gitignore")):equals("")          -- a dotfile is all stem
  t:expect(p.stem("a/b.txt")):equals("b")
  t:expect(p.stem("b.tar.gz")):equals("b.tar")
  t:expect(p.stem(".gitignore")):equals(".gitignore")
  t:expect(p.stem("a/b")):equals("b")
end)

prova.test("path.normalize collapses ./.. and duplicate separators, emitting '/'",
  { proves = "increment-3: prova.path.normalize" }, function(t)
  t:expect(p.normalize("a//b/./c/../d")):equals("a/b/d")
  t:expect(p.normalize("./a")):equals("a")
  t:expect(p.normalize("a/b/")):equals("a/b")       -- trailing slash stripped
  t:expect(p.normalize("../a")):equals("../a")      -- leading .. in a relative path survives
  t:expect(p.normalize("/../a")):equals("/a")       -- .. cannot climb above a root
  t:expect(p.normalize("C:\\Users\\x\\..\\y")):equals("C:/Users/y")
  t:expect(p.normalize("\\\\?\\C:\\a")):equals("C:/a")  -- Windows verbatim prefix dissolves
  t:expect(p.normalize("")):equals(".")
  t:expect(p.normalize("a/..")):equals(".")
end)

prova.test("path.is_absolute recognizes unix, drive, and UNC roots",
  { proves = "increment-3: prova.path.is_absolute" }, function(t)
  t:expect(p.is_absolute("/etc/app")):is_true()
  t:expect(p.is_absolute("C:/Users")):is_true()
  t:expect(p.is_absolute("C:\\Users")):is_true()
  t:expect(p.is_absolute("//server/share")):is_true()
  t:expect(p.is_absolute("a/b")):is_false()
  t:expect(p.is_absolute("./a")):is_false()
end)

prova.test("path claims no ambient global — the bare name stays the user's",
  { proves = "increment-3: prova.path is canonical-only by default" }, function(t)
  -- This suite declares no [globals], so the default inject set applies — which deliberately
  -- excludes high-collision utility names. Canonical access works; the bare name is free.
  ---@diagnostic disable-next-line: undefined-global  (nil-by-default is exactly the claim)
  t:expect(path == nil):is_true()
  t:expect(type(prova.path.join)):equals("function")
  local path = "mine"                               -- and a user local shadows nothing
  t:expect(path):equals("mine")
end)
