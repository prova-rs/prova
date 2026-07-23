# Plan: plugin ecosystem — LANDED (2026-07)

**Folded into [`docs/design/plugin-system.md`](../design/plugin-system.md),
[`docs/design/ecosystem.md`](../design/ecosystem.md), and
[`docs/design/namespacing.md`](../design/namespacing.md).** The extraction shipped: server
databases/brokers left core for pinned plugins (`require("postgres")` …, the
`client`/`container` resource grammar, `prova.containerized`), `[plugins]`/`[sources]` +
profile-scoped `[profiles.X.plugins]` landed in the manifest
([`manifest.md`](../design/manifest.md)), and the shared git cache carries `[updates]`
freshness. This stub remains as the historical pointer.
