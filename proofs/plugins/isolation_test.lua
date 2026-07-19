-- Dogfoods the plugin-composition / isolation contract (the "bundled + isolated" model — see
-- prova-agents@'s docs/design/plugin-system.md). A library plugin (`lib`) privately depends on a
-- provider plugin (`inner`). The invariant: a consumer that requires `lib` can use what `lib` chose to
-- expose, but CANNOT reach `inner` — the inner plugin is lib's private dependency, not part of the
-- consumer's namespace ("if one plugin pulls in another, the inner plugin is never exposed to the
-- caller"). Names are isolated; the caller only sees what it required.
--
-- RED until per-plugin dep maps land (prova-agents@ owns the searcher change). Today every dir under
-- .prova/plugins is a GLOBAL disk root, so `require("inner")` leaks straight through to this consumer —
-- precisely the leak this proof forbids. It goes green when the searcher binds a plugin-scoped
-- `require` (with its own module cache) to lib's `prova-plugin.toml [plugins]`, so `inner` resolves
-- privately for lib and stops resolving here.
local lib = require("lib")

prova.test("a library can use its private dependency internally", function(t)
  -- lib composed inner's value into its own surface — the "a plugin CAN use a dependency" half. This
  -- passes today and must keep passing after isolation lands (don't break what already works).
  t:expect(lib.derived):equals("inner-secret::stamped-by-inner")
end)

prova.test("the library's private dependency is invisible to the consumer", function(t)
  -- The consumer required `lib`, never `inner`. Resolving `inner` from here must FAIL — it is lib's
  -- private dependency, not part of this namespace. RED now (`ok` is true — the global disk root leaks
  -- it); flips to false once isolation lands. (The mechanism must also avoid leaking via the shared
  -- `package.loaded` cache — a plugin-scoped require keeps its own.)
  local ok = pcall(require, "inner")
  t:expect(ok):equals(false)
end)
