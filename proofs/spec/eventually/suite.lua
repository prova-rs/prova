-- THE SPEC for the `:eventually` matcher modifier (docs/plans/api-freeze.md §4) — sugar over
-- prova.retry (which stays public): poll a function subject until its matcher passes. Spec
-- flags are test-level (each open test carries its own); this file only names the suite.
suite.config{ name = "spec-eventually" }
