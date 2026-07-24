-- THE SPEC for the `:eventually` matcher modifier (docs/plans/api-freeze.md §4) — sugar over
-- prova.retry (which stays public): poll a function subject until its matcher passes.
suite.config{ name = "spec-eventually", spec = "api-freeze §4 — :eventually poll-until-matches" }
