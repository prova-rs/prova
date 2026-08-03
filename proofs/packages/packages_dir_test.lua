-- A package can locate ITSELF — the per-package `pkg.dir`.
--
-- The gap this closes was hit driving a real cross-repo integration (Minion consuming Aegis's
-- `aegis` prova package): a package reused via `[dependencies] x = { path = "../other/..." }`
-- needs to find its OWN repo's built binary, but the only anchor it had was `prova.root` — which
-- is the CONSUMING package's root, so it resolved the consumer's `target/`, not its own.
-- `pkg.dir` is the package's real home, always, so `pkg.dir .. "/../../../target/debug/tool"`
-- finds the binary wherever it is consumed.
--
-- (`pkg`, not `package`: that name is Lua's own module table, and a per-chunk shadow of it would
-- take `package.loaded` away from exactly the code that needs `require`. `plugin` is the
-- deprecated alias — same table, retiring at 1.0.)

local shared = require("shared")

prova.test("a package sees its own directory via pkg.dir", function(t)
  -- `shared` captured `pkg.dir` at load. It must be the directory holding the package's own file —
  -- here `<repo>/.prova/packages/shared` — not the project root and not the cwd.
  t:expect(shared.own_dir, "the package's own dir"):never():equals(nil)
  t:expect(shared.own_dir):matches("/%.prova/packages/shared$")
  t:expect(fs.exists(shared.own_dir), "the dir really exists"):equals(true)
end)

prova.test("pkg.dir is the package's home, distinct from prova.root", function(t)
  -- The whole point: `pkg.dir` is anchored on the PACKAGE, `prova.root` on the consuming one.
  -- For a project's own local package they share an ancestor, but the package dir is strictly
  -- deeper — and for a cross-repo package they would be in different repositories entirely.
  t:expect(shared.own_dir):never():equals(prova.root)
  t:expect(shared.own_dir:sub(1, #prova.root), "own_dir is under the project here"):equals(prova.root)
end)

prova.test("the deprecated `plugin` alias still resolves to the same table", function(t)
  t:expect(shared.own_dir_via_alias, "plugin.dir still answers"):equals(shared.own_dir)
end)
