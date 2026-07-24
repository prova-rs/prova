--- `toml` — the dep is already in-tree (manifest parsing); this exposes it to Lua as a module.

prova.test("toml.parse decodes tables and scalars", { spec = "api-freeze §1 - toml module" }, function(t)
  local v = toml.parse('[run]\njobs = 4\nproofs = ["proofs"]\n')
  t:expect(v.run.jobs):equals(4)
  t:expect(v.run.proofs[1]):equals("proofs")
end)

prova.test("toml.encode round-trips parse", { spec = "api-freeze §1 - toml module" }, function(t)
  local v = { package = { name = "demo", port = 8080 } }
  t:expect(toml.parse(toml.encode(v))):equals(v)
end)
