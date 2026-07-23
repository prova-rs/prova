# Plan: snapshot testing — LANDED (phases A+B+C, 2026-07)

**Folded into [`docs/design/architecture.md`](../design/architecture.md) §Snapshots.** Shipped:
colocated reviewable `.snap` files with path+counter keying, `-u/--update-snapshots`,
mismatch line-diffs and `.snap.new` for missing, the `layout`/`content` level dial (anti-rot
default in the API), the generic snapshot protocol, and orphan reconciliation
(`--unreferenced`, full-runs-only soundness). This stub remains as the historical pointer.
