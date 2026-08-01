--- Forward slashes everywhere: every path-PRODUCING API emits `/`-normalized strings — no `\`
--- separators, no `\\?\` verbatim prefix. Backslashed outputs are the root cause of the Windows
--- TOML-escape, shell-quoting, and pattern-match failures, so they are killed at the source.
--- On unix these laws hold trivially; the falsifier is the Windows CI lane, where an unnormalized
--- `C:\Users\…` (or `\\?\C:\…`) output turns every assertion here red.

local p = prova.path

--- The one shape every emitted path must have: absolute, `/`-separated, already in normal form
--- (so `prova.path.normalize` is a no-op on it).
local function expect_normalized(t, s)
  t:expect(s:find("\\", 1, true) == nil):is_true()
  t:expect(p.is_absolute(s)):is_true()
  t:expect(p.normalize(s)):equals(s)
end

prova.test("fs.tempdir emits a /-normalized absolute path",
  { proves = "increment-3b: fs.tempdir output is /-normalized" }, function(t)
  local d = fs.tempdir()
  t:defer(function() fs.remove_all(d) end)
  expect_normalized(t, d)
end)

prova.test("ctx:tempdir emits a /-normalized absolute path",
  { proves = "increment-3b: ctx:tempdir output is /-normalized" }, function(t)
  expect_normalized(t, t:tempdir())
end)

prova.test("fs.glob emits /-normalized absolute paths",
  { proves = "increment-3b: fs.glob output is /-normalized" }, function(t)
  local root = t:tempdir()
  fs.write(root .. "/a/one.txt", "1")
  fs.write(root .. "/a/b/two.txt", "2")
  local hits = fs.glob(root, "**/*.txt")
  t:expect(#hits):equals(2)
  for _, hit in ipairs(hits) do
    expect_normalized(t, hit)
  end
end)
