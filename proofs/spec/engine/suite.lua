-- The spec ENGINE's own black-box surface. The feature was bootstrapped "implemented first,
-- spec'd by hand" (docs/plans/api-freeze.md) — this suite closes that gap: `--specs --list` is
-- proven as a standing guardrail, and the ergonomic burndown verb (`prova --burndown`, subsuming
-- `--specs --strict-specs`) is spec'd ahead of implementation. Spec flags are test-level; this
-- file only names the suite.
suite.config{ name = "spec-engine" }
