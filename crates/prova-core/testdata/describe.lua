-- describe: label-only subgrouping. Bare prova.test inside a top-level describe nests under the
-- label; describes nest; the GroupBuilder form takes a builder. No new fixture scope.

prova.describe("math", function()
  prova.test("adds", function(t)
    t:expect(1 + 1):equals(2)
  end)

  prova.describe("nested", function()
    prova.test("multiplies", function(t)
      t:expect(2 * 3):equals(6)
    end)
    -- test_each also nests under the ambient label.
    prova.test_each("squares {n}", { { n = 2, want = 4 } }, function(t, case)
      t:expect(case.n * case.n):equals(case.want)
    end)
  end)
end)

-- After the describe body returns, the ambient parent pops back to the file root.
prova.test("top level again", function(t)
  t:expect(true):is_truthy()
end)

-- GroupBuilder:describe — a labeling subgroup inside a group, using the builder.
prova.group("outer", function(g)
  g:describe("subsection", function(sub)
    sub:test("runs", function(t)
      t:expect("ok"):equals("ok")
    end)
  end)
end)
