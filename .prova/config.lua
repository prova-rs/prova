-- The runtime companion: loaded once, pre-suite. `runtime.*` is only valid here.
-- A marker capability so a proof can demonstrate the companion is wired.
runtime.capability("prova_selftest", function() return true end)
