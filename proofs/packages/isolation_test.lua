-- Dogfoods the package-composition / isolation contract (the "bundled + isolated" model). A library
-- package (`lib`) privately depends on a provider package (`inner`), declared in lib's own
-- `prova.toml [dependencies]`. The invariant: a consumer that requires `lib` can use what `lib`
-- chose to expose, but CANNOT reach `inner` — the inner package is lib's private dependency, not
-- part of the consumer's namespace ("if one package pulls in another, the inner package is never
-- exposed to the caller"). Names are isolated; the caller only sees what it required.
--
-- Note where `inner` lives: INSIDE lib (`.prova/packages/lib/deps/inner`), not at the top of
-- `.prova/packages/`. A top-level directory there is an ambient package and is globally requirable
-- by design, so parking a private dependency there would leak it to everyone — the layout is part of
-- the contract, not an accident of the fixture.
local lib = require("lib")

prova.test("a library can use its private dependency internally",
  { covers = "docs/design/package-system.md#private-dependencies-isolated" }, function(t)
  -- lib composed inner's value into its own surface. This is the half that proves isolation didn't
  -- simply break composition: the library must still be able to USE what it depends on.
  t:expect(lib.derived):equals("inner-secret::stamped-by-inner")
end)

prova.test("a private dependency required lazily, at test time, still resolves privately",
  { covers = "docs/design/package-system.md#private-dependencies-isolated" }, function(t)
  -- `lib.lazy_secret` runs its require NOW — long after lib's chunk loaded, from a call frame the
  -- consumer owns. It must still hit lib's private map, because the scoping was bound to the chunk's
  -- environment at load, not to whoever happens to be on the stack.
  t:expect(lib.lazy_secret()):equals("inner-secret")
end)

prova.test("the library's private dependency is invisible to the consumer",
  { covers = "docs/design/package-system.md#private-dependencies-isolated" }, function(t)
  -- The consumer required `lib`, never `inner`. Resolving `inner` from here must fail — it is lib's
  -- private dependency, not part of this namespace.
  local ok = pcall(require, "inner")
  t:expect(ok):equals(false)

  -- The subtler leak, worth its own assertion: even with the searcher scoped correctly, caching a
  -- private module in the global `package.loaded` (which is keyed by NAME) would hand every consumer
  -- a reference to it. Private deps are cached by path, registry-side, precisely to avoid this.
  t:expect(package.loaded["inner"]):is_nil()
end)
