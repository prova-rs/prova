# Plan: phase-1 authoring ergonomics — LANDED / RESOLVED (2026-07-15)

**Folded into [`docs/design/api.md`](../design/api.md) §Decision record.** `test_each` +
`describe` shipped; parametrized fixtures (`ctx:param`) and `f:use` were assessed and DROPPED
(action-at-a-distance vs the explicit lazy `t:use` model; flow fixtures are `t:use` inside
steps, scope-cached). `describe_each` stays unbuilt until a real trigger: the same case-list
copied across several `test_each`, or a whole block × N shared-assertion variants — it
composes `describe` + `test_each`, both shipped. This stub remains as the historical pointer.
