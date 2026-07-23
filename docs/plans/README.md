# In-progress plans

Working plans for active, in-flight prova work — distinct from `docs/design/`, which holds the
durable design docs (north-star, architecture, manifest, api, ecosystem, topologies). When a plan
lands, its outcome is folded into the design docs and the plan trimmed to a `LANDED` pointer stub.

> **Repo location note:** the active working copy is `/Users/jimmie/personal/prova-rs/prova-agents`
> (one jj repo, multiple workspaces). Older project memory may still reference earlier checkouts
> (`archetect/prova`, `prova-rs/prova`).

## Active plans

- [autodidact.md](autodidact.md) — **open, drafted 2026-07-21.** The progressive-disclosure
  learning system: `prova skill` as entry/router, `prova learn <topic>` + MCP `learn` tool over
  one embedded topic catalog (static doctrine + dynamic slots rendered per-project), a `context`
  manifest key for project-provided docs, and introspection truth repair (phantom
  `before_each`/`after_*` stubs, plugin APIs invisible to `introspect`, unstubbed
  `prova.workspace`). Enforcement ladder: undocumented features made unrepresentable (topic/slot
  enums, verb table, schema self-docs, registration-carries-docs end-state); same sources later
  export the site's reference pages. *Core rails (learn/introspect/skill/context) shipped; the
  truth-repair and enforcement items remain.*
- [mocks.md](mocks.md) — virtualize the dependency you can't run, and assert on the interactions
  you can't otherwise see. **A (`http.mock`), B (`grpc.mock`), C1 (passthrough/record/replay),
  C2 (network vantage) landed 2026-07-16/17; C3 (alias-interposition shim) + D (`net.mock`) +
  E (`graphql.mock`) open**, each behind a real-consumer trigger. The load-bearing bet held
  twice: a stub's reply can be a Lua function over HTTP/1 *and* HTTP/2 — no response-templating
  language, now or later.
- [parallels.md](parallels.md) — VM-style testing. **(A) the Linux harness — done** (proved C2 on
  a native-Linux VM); **(B) a `parallels.vm(ctx)` resource plugin — deferred** until VM-style
  testing has a real consumer. Records the axis C2 exposed: *where prova runs relative to the
  substrate*.
- [docker-port-binding-investigation.md](docker-port-binding-investigation.md) — investigation
  note (kept for the record; not a feature plan).

## Landed (pointer stubs; content folded into docs/design/)

- [topology.md](topology.md) → [`topologies.md`](../design/topologies.md)
- [plugin-ecosystem.md](plugin-ecosystem.md) → [`plugin-system.md`](../design/plugin-system.md) /
  [`ecosystem.md`](../design/ecosystem.md) / [`namespacing.md`](../design/namespacing.md)
- [snapshots.md](snapshots.md) → [`architecture.md`](../design/architecture.md) §Snapshots
- [phase1-ergonomics.md](phase1-ergonomics.md) → [`api.md`](../design/api.md) §Decision record
- [init-catalog.md](init-catalog.md) → [`ide-and-layout.md`](../design/ide-and-layout.md) §prova init
- [layout.md](layout.md) → [`ide-and-layout.md`](../design/ide-and-layout.md) +
  [`manifest.md`](../design/manifest.md)
