-- Prova's own capability predicates, reached from `.prova.toml`'s `[capabilities]`.
--
-- This package is the dogfood half of docs/design/capabilities.md: the predicate that used to live
-- in the `.prova/config.lua` companion (`runtime.capability("prova_selftest", ...)`) lives here
-- instead, as an ordinary exported function. Two things follow, and the second is the whole point of
-- the move:
--
--   1. `.prova.toml` names it, so the vocabulary is readable in the one file a reader consults.
--   2. A PROOF can call it directly — `require("env").capabilities.selftest()` — which a function
--      inside a file the runtime loaded for itself could never offer.
--
-- Everything under `capabilities` is addressable as `capability = "<key>"`; anything else in this
-- package needs the explicit `factory = "<dotted.path>"` form.
local M = { capabilities = {} }

--- The marker capability. A proof gates on it (proofs/assertions/matchers_test.lua), so a
--- `[capabilities]` vocabulary that stopped resolving shows up as a SKIP in prova's own suite rather
--- than as nothing at all — the same job the companion's marker did, one mechanism later.
---
--- Returns `true` rather than a version: there is no version of "this project's wiring works", and
--- inventing one would make `requires = { "prova_selftest >= 1" }` look meaningful.
function M.capabilities.selftest()
  return true
end

return M
