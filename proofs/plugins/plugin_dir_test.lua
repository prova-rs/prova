-- A plugin can locate ITSELF — the per-plugin `plugin.dir`.
--
-- The gap this closes was hit driving a real cross-repo integration (Minion consuming Aegis's `aegis`
-- prova plugin): a plugin reused via `[plugins] x = { path = "../other/..." }` needs to find its OWN
-- repo's built binary, but the only anchor it had was `prova.root` — which is the CONSUMING package's
-- root, so it resolved the consumer's `target/`, not its own. `plugin.dir` is the plugin's real home,
-- always, so `plugin.dir .. "/../../../target/debug/tool"` finds the binary wherever it is consumed.

local shared = require("shared")

prova.test("a plugin sees its own directory via plugin.dir", function(t)
  -- `shared` captured `plugin.dir` at load. It must be the directory holding the plugin's own file —
  -- here `<repo>/.prova/plugins/shared` — not the project root and not the cwd.
  t:expect(shared.own_dir, "the plugin's own dir"):never():equals(nil)
  t:expect(shared.own_dir):matches("/%.prova/plugins/shared$")
  t:expect(fs.exists(shared.own_dir), "the dir really exists"):equals(true)
end)

prova.test("plugin.dir is the plugin's home, distinct from prova.root", function(t)
  -- The whole point: `plugin.dir` is anchored on the PLUGIN, `prova.root` on the consuming package.
  -- For a project's own plugin they share an ancestor, but the plugin dir is strictly deeper — and
  -- for a cross-repo plugin they would be in different repositories entirely.
  t:expect(shared.own_dir):never():equals(prova.root)
  t:expect(shared.own_dir:sub(1, #prova.root), "own_dir is under the project here"):equals(prova.root)
end)
