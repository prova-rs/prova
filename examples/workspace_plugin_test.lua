-- Loading a plugin with `require`. `prova.workspace` is a bundled first-party plugin resolved by the
-- plugin searcher — the same path a third-party plugin on PROVA_PLUGIN_PATH / .prova/plugins takes.
-- Run: prova examples/workspace_plugin_test.lua

local workspace = require("prova.workspace")

prova.test("scratch workspace is created, written, and auto-removed", function(t)
  local ws = workspace.create(t)
  ws:write("Cargo.toml", "[package]\nname = \"demo\"\n")
  ws:write("src/main.rs", "fn main() { println!(\"hi\"); }")

  t:expect(ws:exists("src/main.rs")):is_true()
  t:expect(ws:read("Cargo.toml")):matches("name")
  -- No teardown needed: ctx:manage removes the directory when the test scope ends.
end)
