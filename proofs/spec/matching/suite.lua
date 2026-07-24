-- The `:matches` structural-subset suite (docs/plans/api-freeze.md §3). The subset matcher is
-- IMPLEMENTED and these are ordinary, line-holding proofs — no suite-level flag remains. The one
-- part still open (the json.null sentinel, blocked on the formats module) carries its own `spec`
-- flag directly: a test is either flagged as a spec, or it is a proof with nothing to indicate.
suite.config{ name = "spec-matching" }
