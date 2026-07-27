-- Journal standardization (docs/plans/api-freeze.md §6): one `received()` vocabulary across
-- every observation journal — seq/source/matched over the transport-native fields, filters
-- accepting the :on shapes (subset table | predicate). IMPLEMENTED and graduated; these run
-- flag-free as guardrails. `prova.double` was the reference; http.mock and grpc.mock converged.
suite.config{ name = "spec-journals" }
