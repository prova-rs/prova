prova.test("b: sees the SAME suite instance (built once across files)", function(t)
  t:expect(t:use("shared")):equals(1)   -- not rebuilt for this file
  t:expect(_G.suite_builds):equals(1)   -- the suite factory ran exactly once
end)
prova.test("b: file fixture rebuilt for the second file", function(t)
  t:expect(t:use("perfile")):equals(2)  -- per-file scope → a fresh build for file b
  t:expect(_G.file_builds):equals(2)
end)
