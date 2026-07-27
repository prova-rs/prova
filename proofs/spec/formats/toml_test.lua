--- `toml` — the dep is already in-tree (manifest parsing); this exposes it to Lua as a module.

prova.test("toml.decode decodes tables and scalars", function(t)
  local v = toml.decode('[run]\njobs = 4\nproofs = ["proofs"]\n')
  t:expect(v.run.jobs):equals(4)
  t:expect(v.run.proofs[1]):equals("proofs")
end)

prova.test("toml.encode round-trips decode", function(t)
  local v = { package = { name = "demo", port = 8080 } }
  t:expect(toml.decode(toml.encode(v))):equals(v)
end)
