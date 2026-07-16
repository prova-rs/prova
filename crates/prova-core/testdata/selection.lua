-- Selection semantics: keywords, tags (own + inherited), exact nodes, dependency pull-in, and
-- flow atomicity.

prova.test("alpha standalone", function(t) t:expect(1):equals(1) end)

local bravo = prova.test("bravo upstream", function(t) t:expect(1):equals(1) end)

prova.test("charlie depends on bravo", { depends_on = { bravo } }, function(t)
  t:expect(2):equals(2)
end)

prova.group("tagged", { tags = { "slow" } }, function(g)
  g:test("delta inherits the slow tag", function(t) t:expect(3):equals(3) end)
end)

prova.test("echo fast", { tags = { "fast" } }, function(t) t:expect(4):equals(4) end)

prova.flow("foxtrot flow", function(f)
  f:step("first step", function(t) t:expect(5):equals(5) end)
  f:step("second step", function(t) t:expect(6):equals(6) end)
end)
