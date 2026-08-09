# In-progress plans

Working plans for active, in-flight prova work — distinct from `docs/design/`, which holds the
durable design docs (north-star, architecture, manifest, api, ecosystem, topologies). When a plan
lands, its outcome is folded into the design docs and the plan trimmed to a `LANDED` pointer stub.

> **Repo location note:** the active working copy is `/Users/jimmie/personal/prova-rs/prova-agents`
> (one jj repo, multiple workspaces). Older project memory may still reference earlier checkouts
> (`archetect/prova`, `prova-rs/prova`).

## Active plans

- [query-consolidation.md](query-consolidation.md) — **drafted 2026-08-08.** The sequel to
  terminology.md: nails the *commands, selectors, and cross-surface parity* on top of the settled
  nouns. Three lanes (specs / tests / reminders), each a medium with a two-state duality; one
  lane-polymorphic query engine with lane + state-filter verbs as sugar; strict-by-default
  capabilities for named lanes (only bare `prova` opportunistic); a new `prova capabilities` verb;
  topology lifecycle unified to one vocabulary across CLI and MCP; and alignment proofs
  (engine ↔ CLI ↔ MCP) as unit tests adopted into the account via junit. Eight proof-carrying
  increments; two open naming sub-decisions (`tests` vs `proofs`; `query` user-facing or engine-only).
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
- [run-progress-feedback.md](run-progress-feedback.md) — **Phase 1 landed 2026-07-27; Phase 2 open.** Turn silent
  run pauses (Docker image pulls foremost, plus seven other intrinsic waits) into understood
  latency. A stderr-only activity side-channel (`trait Progress` in core, terminal renderer in
  cli) — deliberately **not** an `Event` variant, so `--format json`/`tap`/MCP stay untouchable.
  Two concerns split: plain status lines everywhere (LLM-safe) + TTY-only transient spinner/bar.
  Phase 1 (plain threshold-gated lines, no new deps) is the whole "looks hung" fix; Phase 2 is
  TTY enrichment. The pull's per-layer progress is *already produced and discarded* at
  `modules.rs:3298`.

## Landed (pointer stubs; content folded into docs/design/)

- [topology.md](topology.md) → [`topologies.md`](../design/topologies.md)
- [plugin-ecosystem.md](plugin-ecosystem.md) → [`package-system.md`](../design/package-system.md) /
  [`ecosystem.md`](../design/ecosystem.md) / [`namespacing.md`](../design/namespacing.md)
- [snapshots.md](snapshots.md) → [`architecture.md`](../design/architecture.md) §Snapshots
- [phase1-ergonomics.md](phase1-ergonomics.md) → [`api.md`](../design/api.md) §Decision record
- [init-catalog.md](init-catalog.md) → [`ide-and-layout.md`](../design/ide-and-layout.md) §prova init
- [layout.md](layout.md) → [`ide-and-layout.md`](../design/ide-and-layout.md) +
  [`manifest.md`](../design/manifest.md)
