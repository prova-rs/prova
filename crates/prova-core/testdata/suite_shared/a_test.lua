prova.test("a: suite fixture built and cached", function(t)
  t:expect(t:use("shared")):equals(1)
  t:expect(t:use("shared")):equals(1)   -- cached within the suite
end)
prova.test("a: file fixture for the first file", function(t)
  t:expect(t:use("perfile")):equals(1)
end)
