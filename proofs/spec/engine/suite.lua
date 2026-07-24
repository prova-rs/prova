-- The spec ENGINE's own black-box surface. The feature was bootstrapped "implemented first,
-- spec'd by hand" (docs/plans/api-freeze.md) — this suite closes that gap: `--specs --list`,
-- the `prova specs` / `prova burndown` verbs, and the learn topic that teaches them are all
-- held here as standing guardrails.
suite.config{ name = "spec-engine" }
