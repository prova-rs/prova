-- Exercises the plugin searcher: a bundled first-party module and a disk plugin, both via `require`.

-- Bundled: `prova.workspace` is embedded in the binary, resolved with no disk lookup.
local workspace = require("prova.workspace")

-- Disk: `greet` resolves from a declared plugin root (the test passes testdata/plugins via
-- `RunConfig::with_plugin_root` — roots are always declared, never taken from the environment).
local greet = require("greet")

prova.test("disk plugin resolves and runs", function(t)
  t:expect(greet.hello("prova")):equals("hello, prova")
end)

prova.test("bundled workspace plugin manages a scratch dir tied to the test", function(t)
  local ws = workspace.create(t)
  ws:write("src/main.rs", "fn main() {}")
  t:expect(ws:exists("src/main.rs")):is_true()
  t:expect(ws:read("src/main.rs")):equals("fn main() {}")
  -- The directory is removed on teardown by ctx:manage — no manual cleanup here.
end)

prova.test("a missing plugin raises a require error", function(t)
  local ok, err = pcall(require, "nope.not_a_plugin")
  t:expect(ok):is_false()
  t:expect(err):matches("no prova plugin")
end)
