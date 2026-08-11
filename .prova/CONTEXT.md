# House rules — capturing and writing in THIS repo

**Where a capture goes.** `docs/design/` is the writable spec source; pick the doc that owns the
subject: `lifecycle.md` (the obligation model: backlog/claim/promise/proof, vocabulary),
`manifest.md` (prova.toml surface, switches, profiles, runner), `verifiers.md` (deputed evidence,
coverage, baselines/ratchets), `reminders.md` (the future-watching rail), `mcp-mode.md` (the agent
surface: skill, learn, MCP tools), `registry.md` (init catalog + package registries),
`topologies.md`, `mocks-proxies-drivers.md`, `packages.md`, `positioning.md` (the pitch).
In-flight plans live in `docs/plans/` and are NOT spec sources — fold outcomes back into
`docs/design/` when they land.

**The item register.** Backlog and claim items here are essay-form doctrine, not one-liners: a
**bold thesis sentence**, then a dense narrative argument (why now, what breaks without it, what
it touches), cross-referencing related anchors by `path#id`. End every item with
`Recorded YYYY-MM-DD.` — the capture date lives in the prose until
`docs/design/lifecycle.md#anchor-records-when-it-was-captured` lands and makes it the anchor's.

**Spec-first, promise-preferred.** When the contract can be stated as a proof today, author it as
a `promises = "…"` test instead of prose — the red body is the record. Prose anchors are for what
genuinely is not executable yet.

**Repo mechanics that bite.** Version control is jj, never git. The tree is deliberately not
rustfmt-clean — match surrounding style by hand, never `cargo fmt` repo-wide. Proofs are the
quality interface: `prova run --list` for the legs, and extend `proofs/` whenever runtime
behavior changes.
