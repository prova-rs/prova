-- The whole suite requires a capability that isn't present, so every test below skips (not fails).
suite.config{ name = "needs-missing-cap", requires = { "__definitely_not_on_path__" } }
