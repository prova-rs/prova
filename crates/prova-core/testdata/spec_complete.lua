-- When every test under a spec flag has graduated, the flag is dead — the run refuses to proceed
-- until the flag and its `spec = false` markers are removed (the final cleanup commit).
prova.group("done", { spec = "finished feature" }, function(g)
  g:test("first", { spec = false }, function(t)
    t:expect(1):equals(1)
  end)
  g:test("second", { spec = false }, function(t)
    t:expect(1):equals(1)
  end)
end)
