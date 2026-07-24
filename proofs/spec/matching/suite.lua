-- THE SPEC for structural subset matching (docs/plans/api-freeze.md §3), authored ahead of its
-- implementation. Every test in this directory inherits the open flag below; tests pinning
-- already-true behavior are born graduated (`spec = false`). Burn down with:
--   prova --specs --strict-specs proofs/spec
suite.config{ name = "spec-matching", spec = "api-freeze §3 — :matches structural subset" }
