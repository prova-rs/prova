-- Tier-1 proof of the bundled + isolated contract, independent of prova's own repo layout.
--
-- `alpha` and `beta` each privately depend on a *different* plugin that they both call `store`.
-- Everything below follows from names being isolated per-plugin rather than shared.
local alpha = require("alpha")
local beta = require("beta")

prova.test("each library resolves its own private dependency", function(t)
  -- Both said `require("store")`; each got its own. If resolution were global, one would have
  -- shadowed the other (or collided) — the whole reason short names have to be plugin-scoped.
  t:expect(alpha.flavour):equals("alpha's store")
  t:expect(beta.flavour):equals("beta's store")
end)

prova.test("a private dependency is not reachable by the consumer", function(t)
  -- The consumer required `alpha` and `beta`, never `store`. There is no global `store`, and there
  -- is no answer to "which one would it even be" — that ambiguity is precisely why it must not
  -- resolve at all rather than silently pick one.
  local ok = pcall(require, "store")
  t:expect(ok):equals(false)
end)

prova.test("a private dependency does not leak through package.loaded", function(t)
  -- The subtle leak: caching a private module in the global `package.loaded` (keyed by NAME) would
  -- hand every consumer a reference to it, even with the searcher scoped correctly.
  t:expect(package.loaded["store"]):is_nil()
end)
