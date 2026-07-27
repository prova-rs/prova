-- Journal standardization (docs/plans/api-freeze.md §6): one `received()` vocabulary across
-- every observation journal. `prova.double` already implements it and is the reference
-- (guardrails in proofs/doubles/); these specs lift http.mock and grpc.mock to the same shape.
suite.config{ name = "spec-journals" }
