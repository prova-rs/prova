-- Runs only if `config.lua` was loaded from the configured path and registered `greeting_ready`.
-- Without the config key wired, the capability is absent and this SKIPS.
prova.test("gated on a capability from the configured config.lua",
  { requires = { "greeting_ready" } }, function(t)
  t:expect(1):equals(1)
end)
