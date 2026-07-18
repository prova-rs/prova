-- Loaded once, pre-suite, as the companion (via `config = "proofs/shared/config.lua"`).
-- runtime.* is only valid here — this is the project's runtime configuration.
runtime.capability("greeting_ready", function() return true end)
