-- Dogfoods the matcher surface.
prova.test("core matchers behave", function(t)
  t:expect(2 + 2):equals(4)
  t:expect("prova"):contains("rov")
  t:expect({ 1, 2, 3 }):has_length(3)
  t:expect(nil):is_nil()
  t:expect(5):gt(3)
  t:expect("x"):never():equals("y")
end)

prova.test("gated on the companion capability", { requires = { "prova_selftest" } }, function(t)
  t:expect(true):is_true()   -- runs only because config.lua registered `prova_selftest`
end)

-- `exists` is polymorphic: present for whatever the subject IS. It sits next to `is_nil` in every
-- matcher listing, so `expect(value):exists()` is the natural way to write a presence check — and it
-- used to fail, reporting `expected path <table> to exist` about something that was never a path.
-- Same resolution `is_empty` makes, and the string case is resolved the same way: as a path.
prova.test("exists() answers presence for values that are not path-shaped", function(t)
  t:expect({ a = 1 }):exists()
  t:expect({}):exists()          -- an empty table is still PRESENT
  t:expect(0):exists()           -- and 0/false are present, not absent
  t:expect(false):exists()
  t:expect(nil):never():exists()
end)

prova.test("exists() still asks the filesystem for path-shaped subjects", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/present.txt", "x")

  t:expect(dir .. "/present.txt"):exists()
  -- The load-bearing use: a missing file must FAIL, even though the string itself is non-nil.
  t:expect(dir .. "/absent.txt"):never():exists()
  -- A handle carrying `path` resolves the same way.
  t:expect({ path = dir .. "/present.txt" }):exists()
  t:expect({ path = dir .. "/absent.txt" }):never():exists()
end)

prova.test("a separator-less string that misses names the matcher you probably wanted", function(t)
  -- The teaching case: `expect("unused"):exists()` is almost always a presence check written with
  -- the wrong matcher, so the failure must say so rather than leave the author guessing.
  local ok, err = pcall(function()
    t:expect("unused"):exists()
  end)
  local message = ok and (t.failures and t.failures[#t.failures] or "") or tostring(err)
  t:expect(tostring(message)):contains("never():is_nil()")
end)
