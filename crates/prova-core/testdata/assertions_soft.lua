-- Proves expect_all collects failures WITHOUT aborting at the first: the block sets a flag after
-- two failing assertions, and a second test confirms the flag was set (so the block ran to the end).
local reached_end = false

prova.test("expect_all collects failures without aborting early", function(t)
  t:expect_all(function()
    t:expect(1):equals(2)       -- soft failure #1
    t:expect("a"):equals("b")   -- soft failure #2 — still evaluated
    reached_end = true          -- reached only because failures did not abort
  end)
end)

prova.test("the soft block ran to completion", function(t)
  t:expect(reached_end):is_true()
end)
