-- Every new matcher, all passing.
prova.test("deep table equality", function(t)
  t:expect({ a = 1, b = { c = 2, d = { 3, 4 } } }):equals({ a = 1, b = { c = 2, d = { 3, 4 } } })
  t:expect({ 1, 2, 3 }):equals({ 1, 2, 3 })
  t:expect({ a = 1 }):never():equals({ a = 2 })
  t:expect({ a = 1 }):never():equals({ a = 1, b = 2 })  -- extra key on the right
end)

prova.test("value matchers", function(t)
  t:expect(false):is_falsy()
  t:expect(nil):is_falsy()
  t:expect("hello world"):matches("wor%a+")
  t:expect("abc"):never():matches("%d+")
  t:expect("hello"):has_length(5)
  t:expect({ 10, 20, 30 }):has_length(3)
  t:expect("rust"):is_one_of({ "go", "rust", "java" })
  t:expect(200):is_one_of({ 200, 204 })
  t:expect(5):gt(4)
  t:expect(5):gte(5)
  t:expect(5):lt(6)
  t:expect(5):lte(5)
  t:expect(5):never():gt(5)
end)

prova.test("is_empty on dirs and files", function(t)
  local dir = fs.tempdir()
  t:expect(dir):is_empty()                       -- fresh temp dir
  local f = dir .. "/empty.txt"
  fs.write(f, "")
  t:expect(f):is_empty()                         -- zero-byte file
  fs.write(dir .. "/full.txt", "x")
  t:expect(dir .. "/full.txt"):never():is_empty()
  t:expect(dir):never():is_empty()               -- dir now has entries
end)

prova.test("expect_all passes when every soft assertion passes", function(t)
  t:expect_all(function()
    t:expect(1):equals(1)
    t:expect("a"):equals("a")
    t:expect({ 1 }):has_length(1)
  end)
end)
